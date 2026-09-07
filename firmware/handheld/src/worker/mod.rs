//! Worker threads to do background blocking work.

use std::path::PathBuf;
use std::sync::{mpsc, OnceLock};

use crate::core::CoreManager;
use crate::device::drivers::fpga;
use crate::device::Device;
use crate::device::DisplayMode;
use crate::fwinfo::FirmwareVersion;
use crate::input::InputManager;
use crate::{kvs, ui};

#[derive(Debug)]
pub enum Message {
    /// An interrupt request from the FPGA
    FpgaIrq(u32),
    /// The headphone state has changed
    HeadphoneState(bool),
    /// The handheld has docked
    DockBegin {
        serial: u32,
        #[allow(unused)]
        hardware: u32,
        firmware: u32,
    },
    /// The handheld has undocked
    DockEnd,

    /// Run a cartridge
    RunCartridge,
    /// The idle timer has expired
    IdleTimerExpired,

    /// Fetch the list of cores
    FetchCoreList,
    /// Start running a core (ID)
    RunCore(String),
    /// Exit the current core (or cancel loading)
    ExitCore,
    /// A file was selected for the core (could be a directory).
    CoreFileSelected(PathBuf),
    /// Core file selection was cancelled.
    CoreFileCancelled,
    /// Core focus changed
    CoreFocusChanged(bool),
    /// Load core settings
    CoreSettingsLoad,
    /// Core setting changed
    CoreSettingChanged { id: u16, value: u32 },
}

/// Send a message to the worker threads.
pub fn send(message: Message) {
    match SENDER.get() {
        Some(sender) => sender.send(message).unwrap(),
        None => log::error!("Dropping worker message {:?}", message),
    }
}

/// Start the worker threadpool. Called once during system init. Panics if called twice.
pub fn start() {
    let (sender, receiver) = mpsc::channel::<Message>();
    SENDER.set(sender).expect("Worker already initialized");

    // TODO: look into reducing stack usage
    std::thread::Builder::new()
        .name("Worker".to_string())
        .stack_size(16 * 1024)
        .spawn(move || {
            while let Ok(message) = receiver.recv() {
                log::debug!("Dispatch {:?}", message);
                dispatch(message);
            }
        })
        .unwrap();
}

static SENDER: OnceLock<mpsc::Sender<Message>> = OnceLock::new();

fn dispatch(message: Message) {
    match message {
        Message::FpgaIrq(irq_mask) => {
            if (irq_mask & fpga::Irq::ModuleVblank.as_flag()) != 0 {
                // Module vblank
                if let Some(bitstream) = CoreManager::lock().current_bitstream() {
                    bitstream.on_vblank_irq();
                }
            }
        }
        Message::HeadphoneState(has_headphones) => {
            log::info!("Headphone detection: {}", has_headphones);
            let mut device = Device::lock();
            device.dac.set_headphones_enabled(has_headphones).unwrap();
            device.dac.set_speakers_enabled(!has_headphones).unwrap();
        }
        Message::RunCartridge => {
            let cart_type = {
                let mut device = Device::lock();
                device.get_cart_switch()
            };
            log::info!("Cart switch: {}", cart_type);

            let core_id = if cart_type {
                "Game-Bub.GB"
            } else {
                "Game-Bub.GBA"
            };

            CoreManager::lock().run_core(core_id, true);
        }
        Message::DockBegin {
            serial, firmware, ..
        } => {
            ui::send(ui::Message::DockBegin {
                serial: format!("{serial:08X}"),
                firmware: format!("{}", FirmwareVersion::from(firmware)),
            });

            InputManager::lock().remove_all_gamepads();
            let mut device = Device::lock();
            device.docked = true;
            device.change_display_mode(DisplayMode::External).unwrap();
        }
        Message::DockEnd => {
            ui::send(ui::Message::DockEnd);
            InputManager::lock().remove_all_gamepads();
            let mut device = Device::lock();
            device.docked = false;
            device.change_display_mode(DisplayMode::Internal).unwrap();
        }
        Message::IdleTimerExpired => {
            // If the idle timer expires during setup, just power off.
            let setup_stage = kvs::keys::SETUP_STAGE.get().unwrap_or_default();
            if setup_stage == 0 {
                log::warn!("Idle during setup, powering off.");
                Device::lock().power_off();
            }
            // TODO: Dim the screen temporarily.
        }
        Message::FetchCoreList => CoreManager::lock().list_cores(),
        Message::RunCore(id) => CoreManager::lock().run_core(&id, false),
        Message::ExitCore => CoreManager::lock().exit_core(),
        Message::CoreFileSelected(file) => CoreManager::lock().handle_file_selected(file),
        Message::CoreFileCancelled => CoreManager::lock().cancel_file_select(),
        Message::CoreFocusChanged(focused) => CoreManager::lock().focus_changed(focused),
        Message::CoreSettingsLoad => CoreManager::lock().ui_load_settings(),
        Message::CoreSettingChanged { id, value } => CoreManager::lock().setting_changed(id, value),
        #[allow(unreachable_patterns)]
        _ => {
            log::warn!("Unhandled message: {:?}", message);
        }
    }
}
