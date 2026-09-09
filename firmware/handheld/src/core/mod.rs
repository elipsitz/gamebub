use std::{
    fs::File,
    io::{BufReader, BufWriter, ErrorKind, Write},
    ops::DerefMut as _,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use esp_idf_svc::hal::units::Hertz;
use thiserror::Error;

use self::CoreError::*;
use crate::{
    bitstream,
    device::{
        drivers::fpga::{self, SpiCommand, MAX_SPI_READ_CLOCK},
        Device,
    },
    ui,
};
pub use info::*;
use settings::CoreSettings;

mod info;
mod settings;

static CORE_MANAGER: LazyLock<Mutex<CoreManager>> =
    LazyLock::new(|| Mutex::new(CoreManager::new()));

/// Maximum number of files to show at once.
/// TODO: save memory and increase this limit (or avoid altogether)
const FILE_LIST_MAX: usize = 100;

const PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);
const NOTIFY_TIMEOUT: Duration = Duration::from_millis(10);
const SETUP_TIMEOUT: Duration = Duration::from_millis(100);

#[allow(unused)]
mod command {
    // Commands
    pub const GET_STATUS: u32 = 0x0000;
    pub const CORE_RUN: u32 = 0x0100;
    pub const CORE_HALT: u32 = 0x0101;
    pub const SETUP_COMPLETE: u32 = 0x0102;
    pub const NOTIFY_FOCUS: u32 = 0x0200;
    pub const FILE_WRITE_START: u32 = 0x0300;
    pub const FILE_WRITE_END: u32 = 0x0301;
    pub const FILE_READ_START: u32 = 0x0302;
    pub const FILE_READ_END: u32 = 0x0303;

    // Status
    pub const STATUS_UNKNOWN: u32 = 0;
    pub const STATUS_INITIALIZE: u32 = 1;
    pub const STATUS_SETUP: u32 = 2;
    pub const STATUS_CORE_HALT: u32 = 3;
    pub const STATUS_CORE_RUN: u32 = 4;
}

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

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Cannot open file {0}")]
    CannotOpenFile(String),
    #[error("Failed to load file {0}:\n{1}")]
    FailedLoadFile(String, String),
    #[error("Failed to save file {0}:\n{1}")]
    FailedSaveFile(String, String),
    #[error("File {0} wrong size:\nExpected {1} bytes\nActually {2} bytes")]
    FileWrongSize(String, u32, u32),
    #[error("File {0} too big:\nMaximum {1} bytes\nActually {2} bytes")]
    FileTooBig(String, u32, u32),
    #[error("Core command {0:04X} timed out")]
    CommandTimeout(u32),
    #[error("Core command {0:04X} failed")]
    CommandFailed(u32),
    #[error("Timed out waiting for core to be ready")]
    CoreReadyTimeout,
    #[error("{0}")]
    Other(String),
}

/// Core-specific lifecycle callbacks.
///
/// The main purpose is to add core-specific customizations for built-in cores
/// while generic core functionality is being built.
pub trait CoreHandler {
    /// Called after the bitstream is programmed.
    fn on_after_program(&mut self);

    /// Called before loading a file, returns the path override.
    fn get_file_path_override(&mut self, id: u16) -> Option<PathBuf>;

    /// Called before a file is loaded, for any additional stuff.
    fn on_before_file_load(&mut self, id: u16, file: &mut File) -> Result<(), String>;

    /// Called for each chunk of a file loaded
    fn on_during_file_load(&mut self, _id: u16, _data: &[u8]) {}

    /// Called after a file is loaded, for any additional stuff.
    fn on_after_file_load(&mut self, _id: u16) {}

    fn on_before_run(&mut self) -> Result<(), String>;

    fn on_focus_changed(&mut self, has_focus: bool);

    /// Called before saving a file. May override the core's file size.
    fn get_file_size(&mut self, id: u16) -> Option<u32>;

    /// Called after file is written, to write any additional data.
    fn on_after_file_save(&mut self, id: u16, file: &mut File) -> Result<(), String>;

    /// Temporary: as Bitstream trait
    fn as_legacy_bitstream(&mut self) -> &mut dyn bitstream::Bitstream;

    /// Load initial core settings values
    fn load_settings(&mut self) -> Vec<(u16, u32)>;

    /// Called when a core setting value is updated
    fn on_setting_changed(&mut self, id: u16, value: u32);
}

enum CoreHandlerImpl {
    None,
    Gameboy(crate::bitstream::gameboy::Gameboy),
    Gba(crate::bitstream::gba::Gba),
}

impl CoreHandlerImpl {
    fn get_mut(&mut self) -> Option<&mut dyn CoreHandler> {
        match self {
            CoreHandlerImpl::None => None,
            CoreHandlerImpl::Gameboy(gameboy) => Some(gameboy),
            CoreHandlerImpl::Gba(gba) => Some(gba),
        }
    }
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
                // TODO show error to user?
                log::error!("Error loading core '{id}': {e}");
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

        Device::lock().set_cart_power(false);
    }

    fn poll_core_status(&mut self, expected: u32, timeout: Duration) -> Result<(), CoreError> {
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                return Err(CoreError::CoreReadyTimeout);
            }
            let mut status = [0u32];
            self.run_core_command(&[command::GET_STATUS], &mut status, timeout)
                .map_err(|_| CoreError::CommandFailed(command::GET_STATUS))?;

            if status[0] == expected {
                return Ok(());
            }
            std::thread::sleep(CORE_POLL_INTERVAL);
        }
    }

    fn run_core_command(
        &self,
        request: &[u32],
        response: &mut [u32],
        timeout: Duration,
    ) -> Result<(), CoreError> {
        assert!(request.len() > 0);
        // Write command and arguments
        {
            let mut device = Device::lock();
            for (i, &x) in request.iter().enumerate() {
                let _ = device
                    .fpga
                    .write_u32(fpga::REG_CMD_HOST_BASE + (4 * i as u32), x);
            }
            let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_HOST, 0b1000);
        }

        // Poll for command completion.
        let start = Instant::now();
        let status = loop {
            if start.elapsed() > timeout {
                log::error!("Command {:08X} timed out", request[0]);
                return Err(CoreError::CommandTimeout(request[0]));
            }
            let mut device = Device::lock();
            let status = device.fpga.read_u32(fpga::REG_CTRL_CMD_HOST).unwrap();
            if (status & 0b0011) != 0 {
                break status;
            }
            drop(device);
            std::thread::sleep(CORE_POLL_INTERVAL);
        };

        // Read result values and end command
        {
            let mut device = Device::lock();
            for (i, x) in response.iter_mut().enumerate() {
                *x = device
                    .fpga
                    .read_u32(fpga::REG_CMD_HOST_BASE + (4 * i as u32))
                    .unwrap();
            }

            let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_HOST, 0);
        }

        // Check status.
        if (status & 0b0001) == 0 {
            Ok(())
        } else {
            Err(CoreError::CommandFailed(request[0]))
        }
    }

    pub fn focus_changed(&mut self, has_focus: bool) {
        {
            let mut device = Device::lock();
            device.dac.set_power_down(0, !has_focus).expect("DAC power");
            let _ = device
                .fpga
                .write_u32(fpga::REG_CTRL_FOCUS, has_focus as u32);
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

    fn next_file_select(&mut self) {
        let core = self.core_info.as_ref().unwrap();
        let index = loop {
            // Find the index of the next file to load.
            let index = match self.stage {
                Stage::LoadInit => 0,
                Stage::LoadSelectFile(i) => i + 1,
                _ => panic!(),
            };
            if index >= core.files.len() {
                log::info!("File selection complete");
                self.stage = Stage::LoadBitstream;
                self.finish_loading();
                return;
            }
            self.stage = Stage::LoadSelectFile(index);
            let file = &core.files[index];
            if !file.user_selected {
                continue;
            }
            if self.run_cartridge && (file.id == 0 || file.dependent_on_0) {
                // TODO: validate and make sure there's a file 0
                continue;
            }
            break index;
        };
        let file = &core.files[index];

        // Get the starting path for the file browser.
        let last_path = self
            .core_settings
            .as_ref()
            .and_then(|x| x.file_paths.iter().find(|(id, _)| *id == file.id))
            .map(|(_, path)| path.as_path());
        let last_dir = last_path.and_then(|p| p.parent());

        // Use the last path, or the last dir, or finally the root path.
        let (initial_path, initial_dir) = match (last_path, last_dir) {
            (Some(file), Some(parent)) if file.is_file() => (file, parent),
            (_, Some(parent)) if parent.is_dir() => (parent, parent),
            _ => (Path::new(DIR_SDCARD), Path::new(DIR_SDCARD)),
        };

        ui::send(ui::Message::CoreFileSelectBegin {
            label: file.label.to_string(),
            path: initial_path.to_path_buf(),
        });
        self.send_core_file_list(initial_dir);
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
            core_handler
                .on_before_run()
                .map_err(|err| CoreError::Other(err))?;
        }

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

    fn load_files(&mut self) -> Result<(), CoreError> {
        // TODO: Sum of size of files to load (rather than doing it file-by-file).
        let mut overall_transferred = 0u64;
        let mut overall_total = 0u64;
        let mut last_progress_update = Instant::now();

        let mut scratch = crate::bitstream::SCRATCH.take().expect("scratch buffer");

        let core = self.core_info.as_ref().unwrap();
        let file_0_index = core.files.iter().position(|f| f.id == 0);
        for (i, info) in core.files.iter().enumerate() {
            if self.run_cartridge && (info.id == 0 || info.dependent_on_0) {
                continue;
            }

            // Get the file path
            let mut path = self.selected_files[i].clone();

            if let Some(filename) = info.filename.as_ref() {
                // Construct path based on core directory
                path = Some(core.core_dir.join(filename));
            } else if info.dependent_on_0 {
                // Construct a new path based on file 0's path
                let extension = &info.extensions[0][1..]; // Remove the dot
                let file_0_index = file_0_index.unwrap();
                path = self.selected_files[file_0_index]
                    .as_ref()
                    .map(|p| p.with_extension(extension));
            }

            // Possibly override the path
            if let Some(core_handler) = self.core_handler.get_mut() {
                path = path.or(core_handler.get_file_path_override(info.id));
            }

            let path = match path {
                Some(path) => path,
                // Use an empty path. Open will fail and we'll clear it.
                None if info.optional => PathBuf::new(),
                // It shouldn't be possible to have no path for a required file.
                None => panic!("Missing path for required file {}", info.label),
            };

            log::info!("Load file {} from {}", info.label, path.display());

            let file = File::open(&path);
            self.selected_files[i] = Some(path);
            let mut file = match file {
                Ok(file) => file,
                Err(_) if info.optional && info.initialize => {
                    log::info!("Failed to open file, clearing");
                    self.clear_file_slot(info, &mut scratch)?;
                    continue;
                }
                Err(_) if info.optional => {
                    log::info!("Failed to open file, skipping");
                    continue;
                }
                Err(_) => {
                    return Err(CannotOpenFile(info.label.to_string()));
                }
            };

            self.run_core_command(
                &[command::FILE_WRITE_START, info.id as u32],
                &mut [],
                SETUP_TIMEOUT,
            )?;

            if let Some(core_handler) = self.core_handler.get_mut() {
                core_handler
                    .on_before_file_load(info.id, &mut file)
                    .map_err(|e| FailedLoadFile(info.label.to_string(), e))?;
            }
            let file_size = file.metadata().unwrap().len();
            overall_total += file_size;

            if info.exact_size != 0 && file_size != (info.exact_size as u64) {
                return Err(FileWrongSize(
                    info.label.to_string(),
                    info.exact_size,
                    file_size as u32,
                ));
            }
            if info.max_size != 0 && file_size > (info.max_size as u64) {
                return Err(FileTooBig(
                    info.label.to_string(),
                    info.max_size,
                    file_size as u32,
                ));
            }

            let start_time = Instant::now();
            let mut transfer_duration = Duration::ZERO;
            let mut handler_duration = Duration::ZERO;
            let mut transferred = 0;
            // TODO: maybe only bother with background I/O for a large file (> 256KB?)
            let result = crate::util::background_io::iter_chunks(file, &mut scratch, |chunk| {
                let transfer_start = Instant::now();
                let max_clock = Some(Hertz(info.max_transfer_speed * 1000 * 2));
                let command = SpiCommand::new(info.transfer_word_size);
                Device::lock()
                    .fpga
                    .spi_write(max_clock, command, info.address + transferred, chunk)
                    .unwrap();
                transferred += chunk.len() as u32;
                overall_transferred += chunk.len() as u64;
                transfer_duration += transfer_start.elapsed();

                let handler_start = Instant::now();

                if let Some(core_handler) = self.core_handler.get_mut() {
                    core_handler.on_during_file_load(info.id, chunk);
                }
                handler_duration += handler_start.elapsed();

                // Update UI progress bar.
                if last_progress_update.elapsed() > PROGRESS_UPDATE_INTERVAL {
                    let progress = (overall_transferred as f32) / (overall_total as f32);
                    ui::send(ui::Message::CoreLoadProgress(progress));
                    last_progress_update = Instant::now();
                }
            });
            let read_duration = result
                .map_err(|_| FailedLoadFile(info.label.to_string(), "I/O error".to_string()))?;

            let duration = start_time.elapsed();
            self.run_core_command(
                &[
                    // Command
                    command::FILE_WRITE_END,
                    // Arg 1: file ID
                    info.id as u32,
                    // Arg 2: file size (bytes)
                    transferred as u32,
                    // Arg 3: reserved (0)
                    0,
                ],
                &mut [],
                NOTIFY_TIMEOUT,
            )?;
            if let Some(core_handler) = self.core_handler.get_mut() {
                core_handler.on_after_file_load(info.id);
            }

            log::info!(
                "Loaded {} bytes in {} ms ({}/{}/{} ms read/transfer/handler)",
                transferred,
                duration.as_millis(),
                read_duration.as_millis(),
                transfer_duration.as_millis(),
                handler_duration.as_millis(),
            );
        }

        Ok(())
    }

    fn persist_files(&mut self) -> Result<(), CoreError> {
        assert!(self.stage == Stage::Running);
        let core = self.core_info.as_ref().unwrap();
        for (i, info) in core.files.iter().enumerate() {
            if info.read_only {
                continue;
            }
            if self.run_cartridge && (info.id == 0 || info.dependent_on_0) {
                continue;
            }

            let path = self.selected_files[i].clone().unwrap();
            log::info!("Saving file {} to {}", info.label, path.display());

            let mut file_size = 0u32;
            self.run_core_command(
                &[command::FILE_READ_START, info.id as u32],
                std::slice::from_mut(&mut file_size),
                SETUP_TIMEOUT,
            )?;
            if let Some(override_size) = self
                .core_handler
                .get_mut()
                .and_then(|x| x.get_file_size(info.id))
            {
                file_size = override_size;
            }

            let mut file = File::create(path)
                .map_err(|_| FailedSaveFile(info.label.to_string(), "Open failed".to_string()))?;
            let mut scratch = crate::bitstream::SCRATCH.take().expect("scratch buffer");
            let buf = scratch.deref_mut();
            let mut address: u32 = info.address;
            let mut bytes_left = file_size as usize;

            let start_time = Instant::now();
            while bytes_left > 0 {
                let to_read = bytes_left.min(buf.len());
                let data = &mut buf[0..to_read];

                let max_clock =
                    Some(Hertz(info.max_transfer_speed * 1000 * 2).min(MAX_SPI_READ_CLOCK));
                let command = SpiCommand::new(info.transfer_word_size);
                let _ = Device::lock()
                    .fpga
                    .spi_read(max_clock, command, address, data);
                file.write(data).map_err(|_| {
                    FailedSaveFile(info.label.to_string(), "Write failed".to_string())
                })?;
                address += to_read as u32;
                bytes_left -= to_read;
            }
            log::info!(
                "Saved {} bytes in {}ms",
                file_size,
                start_time.elapsed().as_millis() as u32
            );

            self.run_core_command(
                &[command::FILE_READ_END, info.id as u32],
                &mut [],
                NOTIFY_TIMEOUT,
            )?;
            if let Some(core_handler) = self.core_handler.get_mut() {
                core_handler
                    .on_after_file_save(info.id, &mut file)
                    .map_err(|e| FailedSaveFile(info.label.to_string(), e))?;
            }
        }
        Ok(())
    }

    /// Called to persist core settings to the core settings JSON file
    pub fn persist_settings(&mut self) -> Result<(), std::io::Error> {
        assert!(self.stage == Stage::Running);
        let core = self.core_info.as_ref().unwrap();
        let settings = self.core_settings.as_mut().unwrap();
        if core.is_built_in {
            // Built-in cores don't use these settings, avoid confusion by not saving them.
            settings.settings.clear();
        }

        if !std::fs::exists(DIR_SETTINGS)? {
            std::fs::create_dir(DIR_SETTINGS)?;
        }
        let file = File::create(core.get_settings_path())?;
        let writer = BufWriter::with_capacity(256, file);
        serde_json::to_writer(writer, settings)
            .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
    }

    /// Called when a file has been selected for the core.
    /// This could be a file or a directory.
    pub fn handle_file_selected(&mut self, path: PathBuf) {
        if path.is_file() {
            log::info!("File selected: {}", path.display());
            let Stage::LoadSelectFile(file_index) = self.stage else {
                panic!()
            };

            // Update settings
            let settings = self.core_settings.as_mut().unwrap();
            let file_id = self.core_info.as_ref().unwrap().files[file_index].id;
            settings.file_paths.retain(|x| x.0 != file_id);
            settings.file_paths.push((file_id, path.clone()));

            self.selected_files[file_index] = Some(path);
            self.next_file_select();
        } else if path.is_dir() {
            self.send_core_file_list(&path);
        }
    }

    /// Called if a file selection is cancelled.
    pub fn cancel_file_select(&mut self) {
        // TODO support multiple file select (go back to previous file)
        log::info!("File select cancelled");
        self.core_info = None;
        self.stage = Stage::Idle;
    }

    fn send_core_file_list(&self, path: &Path) {
        let files = match self.list_core_files(&path) {
            Ok(files) => files,
            Err(e) => {
                log::warn!("Error listing directory: {:?}", e);
                ui::send(ui::Message::CoreFileSelectError(format!(
                    "Error listing directory:\n{}",
                    e,
                )));
                Vec::new()
            }
        };
        let truncated = files.len() >= FILE_LIST_MAX;
        ui::send(ui::Message::CoreFileSelectList(files));
        if truncated {
            let message =
                format!("More than {FILE_LIST_MAX} items in directory:\nsome items will be hidden");
            ui::send(ui::Message::CoreFileSelectError(message));
        }
    }

    /// Get the list of eligible files for the file select menu at the given directory
    fn list_core_files(&self, path: &Path) -> std::io::Result<Vec<(String, bool)>> {
        // Assumes we're in a valid file selection stage.
        let file_index = match self.stage {
            Stage::LoadSelectFile(i) => i,
            _ => panic!(),
        };
        let extensions = &self.core_info.as_ref().unwrap().files[file_index].extensions;

        let mut files = path
            .read_dir()?
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name();
                let name = name.to_str()?;
                let kind = e.metadata().ok()?.file_type();
                if name.starts_with(".") {
                    return None;
                }
                if kind.is_file() && !extensions.iter().any(|&e| name.ends_with(e.as_str())) {
                    return None;
                }

                Some((name.to_string(), kind))
            })
            .take(FILE_LIST_MAX)
            .collect::<Vec<_>>();
        files.sort_unstable_by(|f1, f2| {
            // Sort by name, with directories first.
            let c1 = (f1.1.is_file(), f1.0.as_str());
            let c2 = (f2.1.is_file(), f2.0.as_str());
            c1.cmp(&c2)
        });
        let files = files.into_iter().map(|f| (f.0, f.1.is_dir())).collect();
        Ok(files)
    }

    /// Clear a file slot to 0xFF
    fn clear_file_slot(&self, info: &CoreFile, buf: &mut [u8]) -> Result<(), CoreError> {
        self.run_core_command(
            &[command::FILE_WRITE_START, info.id as u32],
            &mut [],
            SETUP_TIMEOUT,
        )?;

        buf.fill(0xFF);
        let mut pos = 0u32;
        let len = info.max_size.max(info.exact_size);
        while pos < len {
            let n = ((len - pos) as usize).min(buf.len());
            let max_clock = Some(Hertz(info.max_transfer_speed * 1000 * 2));
            let command = SpiCommand::new(info.transfer_word_size);
            let _ =
                Device::lock()
                    .fpga
                    .spi_write(max_clock, command, info.address + pos, &buf[..n]);
            pos += n as u32;
        }

        self.run_core_command(
            &[
                // Command
                command::FILE_WRITE_END,
                // Arg 1: file ID
                info.id as u32,
                // Arg 2: file size (bytes)
                len,
                // Arg 3: reserved (0)
                0,
            ],
            &mut [],
            NOTIFY_TIMEOUT,
        )?;

        Ok(())
    }
}
