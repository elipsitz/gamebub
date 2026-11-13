use std::{
    fs::File,
    io::BufReader,
    path::PathBuf,
    sync::{LazyLock, Mutex, MutexGuard},
    time::Duration,
};

use crate::{
    bitstream,
    device::{drivers::fpga, Device},
    input::InputManager,
    ui,
};

pub use error::*;
pub use files_on_demand::OnDemandFile;
pub use handler::CoreHandler;
use handler::CoreHandlerImpl;
pub use info::*;
use settings::CoreSettings;

mod command;
mod error;
mod file_selection;
mod files;
mod files_on_demand;
mod handler;
pub mod info;
pub mod settings;

static CORE_MANAGER: LazyLock<Mutex<CoreManager>> =
    LazyLock::new(|| Mutex::new(CoreManager::new()));

const PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);
const NOTIFY_TIMEOUT: Duration = Duration::from_millis(10);
const SETUP_TIMEOUT: Duration = Duration::from_millis(100);

pub const DIR_SDCARD: &str = "/sdcard/";
pub const DIR_SETTINGS: &str = "/sdcard/settings/";
pub const CORE_POLL_INTERVAL: Duration = Duration::from_millis(5);

pub struct CoreManager {
    stage: Stage,
    listed_cores: bool,
    core_info: Option<info::CoreInfo>,
    core_handler: CoreHandlerImpl,
    core_settings: Option<CoreSettings>,
    selected_files: Vec<Option<PathBuf>>,
    /// The currently open on-demand file, if any.
    open_on_demand_file: Option<OnDemandFile>,
    run_cartridge: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Stage {
    Idle,
    LoadInit,
    LoadSelectFile(usize),
    LoadBitstream,
    Running,
}

/// # CoreManager
///
/// Manages the lifecycle of cores.
impl CoreManager {
    fn new() -> Self {
        CoreManager {
            core_info: None,
            listed_cores: false,
            stage: Stage::Idle,
            core_handler: CoreHandlerImpl::None,
            core_settings: None,
            selected_files: Vec::new(),
            open_on_demand_file: None,
            run_cartridge: false,
        }
    }

    pub fn lock() -> MutexGuard<'static, Self> {
        CORE_MANAGER.lock().unwrap()
    }

    /// Find and list all cores (UI)
    pub fn list_cores(&mut self) {
        if self.listed_cores {
            // Already sent the list to the UI, ignore duplicate request.
            return;
        }

        let list = info::list_cores();
        ui::send(ui::Message::CoreList(list));
    }

    /// Start the process of running a specific core (by ID).
    pub fn run_core(&mut self, id: &str, run_cartridge: bool) {
        log::info!("Run core={id} cart={run_cartridge}");
        assert!(self.core_info.is_none());
        assert!(self.stage == Stage::Idle);
        let core = match info::get_core(id) {
            Ok(core) => {
                self.core_info = Some(core);
                self.core_info.as_ref().unwrap()
            }
            Err(e) => {
                log::error!("Error loading core '{id}': {e}");
                ui::send(ui::Message::CoreLoadError(e));
                return;
            }
        };

        self.core_handler = match core.id.as_str() {
            "Game-Bub.GB" => CoreHandlerImpl::Gameboy(crate::bitstream::gameboy::Gameboy::new()),
            "Game-Bub.GBA" => CoreHandlerImpl::Gba(crate::bitstream::gba::Gba::new()),
            _ => CoreHandlerImpl::None,
        };

        self.stage = Stage::LoadInit;
        self.run_cartridge = run_cartridge;
        self.core_settings = Some(Self::load_settings(core));
        if let Some(core_handler) = self.core_handler.get_mut() {
            self.core_settings.as_mut().unwrap().settings = core_handler.load_settings();
        }

        self.selected_files = vec![None; core.files.len()];
        self.next_file_select();
    }

    fn load_settings(core: &CoreInfo) -> CoreSettings {
        fn load(core: &CoreInfo) -> Option<CoreSettings> {
            let file = match File::open(core.get_settings_path()) {
                Ok(file) => file,
                Err(_) => {
                    log::warn!("Failed to open settings file");
                    return None;
                }
            };
            let reader = BufReader::with_capacity(256, file);
            let settings: CoreSettings = match serde_json::from_reader(reader) {
                Ok(x) => x,
                Err(_) => {
                    log::warn!("Failed to parse settings file");
                    return None;
                }
            };
            Some(settings)
        }
        let mut settings = load(core).unwrap_or_default();

        // Only keep file paths that belong to user-selected files.
        settings
            .file_paths
            .retain(|(i, _)| core.files.iter().any(|f| f.id == *i && f.user_selected));

        // Validate and apply defaults.
        settings.settings = core
            .settings
            .iter()
            .map(|setting| {
                let value = settings.settings.iter().find(|x| x.0 == setting.id).map_or(
                    setting.default,
                    |x| {
                        // TODO: check if this is actually a valid setting?
                        x.1
                    },
                );
                (setting.id, value)
            })
            .collect();

        settings
    }

    /// Temporary transitional method
    /// TODO: remove
    pub fn current_bitstream(&mut self) -> Option<&mut dyn bitstream::Bitstream> {
        self.core_handler.get_mut().map(|c| c.as_legacy_bitstream())
    }

    pub fn prepare_for_power_off(&mut self) {
        if self.stage == Stage::Running {
            if let Err(e) = self.persist_files() {
                log::error!("Error saving: {}", e);
            }
            if let Err(e) = self.persist_settings() {
                log::error!("Error saving settings: {}", e);
            }
        }
        self.reset_state();
    }

    pub fn exit_core(&mut self) {
        if self.stage == Stage::Running {
            let _ = self.run_core_command(&[command::CORE_HALT], &mut [], NOTIFY_TIMEOUT);

            if let Err(e) = self.persist_files() {
                log::error!("Error saving: {}", e);
            }
            if let Err(e) = self.persist_settings() {
                log::error!("Error saving settings: {}", e);
            }
        }

        self.reset_state();

        // Power off the DAC
        Device::lock()
            .dac
            .set_power_down(0, true)
            .expect("DAC power");

        // And go back to the boot bitstream
        bitstream::program_boot();
    }

    fn reset_state(&mut self) {
        self.core_info = None;
        self.core_handler = CoreHandlerImpl::None;
        self.core_settings = None;
        self.stage = Stage::Idle;
        self.selected_files.clear();
        self.open_on_demand_file = None;

        InputManager::lock().clear_core_input_caps();

        let _ = Device::lock()
            .fpga
            .disable_interrupt(fpga::Irq::CoreRequest);
        Device::lock().set_cart_power(false);
    }

    pub fn reset_core(&mut self) {
        if self.stage != Stage::Running {
            return;
        }
        let _ = self.run_core_command(&[command::CORE_HALT], &mut [], NOTIFY_TIMEOUT);
        let _ = self.run_core_command(&[command::CORE_RUN], &mut [], NOTIFY_TIMEOUT);
    }

    pub fn focus_changed(&mut self, has_focus: bool) {
        {
            let mut device = Device::lock();
            device.dac.set_power_down(0, !has_focus).expect("DAC power");
            let _ = device
                .fpga
                .write_u32(fpga::REG_CTRL_FOCUS, has_focus as u32);

            // Darken video if not focused
            let video_filter = if has_focus { 0 } else { 1 };
            let _ = device
                .fpga
                .write_u32(fpga::REG_CTRL_VIDEO_FILTER, video_filter);
        }

        let _ = self.run_core_command(
            &[command::NOTIFY_FOCUS, has_focus as u32],
            &mut [],
            NOTIFY_TIMEOUT,
        );

        if let Some(core_handler) = self.core_handler.get_mut() {
            core_handler.on_focus_changed(has_focus);
        }
    }

    fn finish_loading(&mut self) {
        if let Err(e) = self.finish_loading_inner() {
            self.exit_core();
            ui::send(ui::Message::CoreLoadError(e.to_string()));
        }
    }

    fn finish_loading_inner(&mut self) -> Result<(), CoreError> {
        self.load_bitstream();

        // Wait for the core to be ready for setup.
        self.poll_core_status(command::STATUS_SETUP, SETUP_TIMEOUT)?;
        ui::send(ui::Message::EnterGame);

        // Read core runtime metadata
        let caps = Device::lock()
            .fpga
            .read_u32(fpga::REG_INFO_INPUT_CAPS)
            .unwrap();
        InputManager::lock().set_core_input_caps(caps);

        let cart_power = match self.core_info.as_ref().unwrap().uses_cartridge {
            CoreCartridgeMode::No => false,
            CoreCartridgeMode::Yes => true,
            CoreCartridgeMode::IfSelected => self.run_cartridge,
        };
        if cart_power {
            let mut device = Device::lock();
            // Power turned off in reset_state.
            device.set_cart_power(true);
        }

        self.load_files()?;

        // Transmit initial settings values
        {
            let info = self.core_info.as_ref().unwrap();
            let settings = &self.core_settings.as_ref().unwrap().settings;
            for &(id, value) in settings {
                let setting = match info.settings.iter().find(|x| x.id == id) {
                    Some(x) => x,
                    None => continue,
                };
                if matches!(setting.inner, CoreSettingType::Action { .. }) {
                    // Actions do not get an initial value
                    continue;
                }

                if let Some(core_handler) = self.core_handler.get_mut() {
                    core_handler.on_setting_changed(id, value);
                }
                self.setting_send(setting, value);
            }
        }

        if let Some(core_handler) = self.core_handler.get_mut() {
            core_handler.on_before_run().map_err(CoreError::Other)?;
        }

        // Start listening for core requests
        let _ = Device::lock().fpga.enable_interrupt(fpga::Irq::CoreRequest);

        // Tell the core we're finished setting up.
        self.run_core_command(&[command::SETUP_COMPLETE], &mut [], NOTIFY_TIMEOUT)?;

        // Wait for the core to be ready for run.
        self.poll_core_status(command::STATUS_CORE_HALT, SETUP_TIMEOUT)?;
        log::info!("Core ready to run");

        // Clear loading bar
        ui::send(ui::Message::EnterGame);
        self.stage = Stage::Running;

        // Start core.
        let _ = Device::lock().fpga.write_u32(fpga::REG_CTRL_VIBRATE, 1);

        let _ = self.run_core_command(&[command::CORE_RUN], &mut [], NOTIFY_TIMEOUT);
        self.focus_changed(true);
        Ok(())
    }

    fn load_bitstream(&mut self) {
        assert!(self.stage == Stage::LoadBitstream);

        bitstream::program_fpga(&self.core_info.as_ref().unwrap().bitstream);

        if let Some(core_handler) = self.core_handler.get_mut() {
            core_handler.on_after_program();
        }
    }
}
