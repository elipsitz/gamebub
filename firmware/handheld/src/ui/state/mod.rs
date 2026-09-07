use std::{cell::RefCell, collections::VecDeque, path::PathBuf, rc::Rc, time::Duration};

use slint::{ComponentHandle, Global, Timer, TimerMode, Weak};

use crate::kvs;
use crate::{core::CoreManager, device::Device};

use super::slint::{
    Backend, MainWindow, ScreenId, SettingDatetime, SettingEntry, SettingType, SettingValue,
};

mod cores;
mod game;
mod main_menu;
pub mod notifications;
mod settings;
mod setup;
mod tools;

pub use game::CoreSettingUiItem;

pub struct UiState {
    root: Weak<MainWindow>,

    notification_timer: Timer,
    notification_queue: VecDeque<notifications::Notification>,
    notification_active: bool,

    settings: settings::SettingsState,
    core_file_select_directory: PathBuf,
    core_file_select_filename: String,
    core_file_select_timer: Timer,
}

impl UiState {
    pub fn new(root: &MainWindow, device: &mut Device) -> Rc<RefCell<Self>> {
        let state: UiState = UiState {
            root: root.as_weak(),
            notification_timer: Timer::default(),
            notification_queue: VecDeque::new(),
            notification_active: false,
            settings: settings::SettingsState::default(),
            core_file_select_directory: PathBuf::new(),
            core_file_select_filename: String::new(),
            core_file_select_timer: Timer::default(),
        };
        let state = Rc::new(RefCell::new(state));
        state.borrow_mut().setup(state.clone(), device);
        state
    }

    pub fn update_battery_level(&mut self, level: f32) {
        let level = level.round() as i32;
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_battery_level(level);
    }

    fn setup(&mut self, state: Rc<RefCell<UiState>>, device: &mut Device) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        let battery_level = device.fuel_gauge.get_battery_level().unwrap_or(0.0);
        self.update_battery_level(battery_level);
        backend.set_volume_level(((kvs::keys::VOLUME.get().unwrap() as i32) * 100) / 255);
        backend.set_brightness_level((kvs::keys::BRIGHTNESS.get().unwrap() * 100.0) as i32);
        backend.set_hardware_version(crate::hwinfo::get_hardware_version().to_string().into());
        backend.set_hardware_serial(crate::hwinfo::get_serial_number().to_string().into());
        backend.set_firmware_version(env!("CARGO_PKG_VERSION").into());
        backend.set_sdcard_mounted(std::fs::exists("/sdcard").unwrap_or(false));

        self.setup_setup(&state, device);
        self.setup_main_menu(&state, device);
        self.setup_game(&state, device);
        self.setup_tools(&state, device);
        self.setup_settings(&state, device);
        self.setup_cores(&state, device);

        // Check for first boot
        if kvs::keys::SETUP_STAGE.get().unwrap_or_default() == 0 {
            root.invoke_set_screen(ScreenId::Setup);
        }

        // Firmware update notification.
        {
            let last_fw = kvs::keys::LAST_FIRMWARE_VERSION.get();
            if last_fw.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
                kvs::keys::LAST_FIRMWARE_VERSION.set(&env!("CARGO_PKG_VERSION").to_string());

                if last_fw.is_some() {
                    let notification = notifications::Notification::new_long(format!(
                        "Firmware updated to {}",
                        env!("CARGO_PKG_VERSION")
                    ));
                    self.queue_notification(state.clone(), notification);
                }
            }
        }

        let state_ = state.clone();
        backend.on_screen_enter(move |screen| {
            // Called when a new screen is entered, before the new frame is rendered.
            log::info!("Screen enter: {:?}", screen);
            match screen {
                ScreenId::Settings => state_.borrow_mut().on_settings_enter(),
                _ => {}
            }
        });

        backend.on_volume_changed({
            let state_ = state.clone();
            let timer = Timer::default();
            move |value| {
                let volume = ((value * (u8::MAX as i32)) / 100) as u8;
                Device::lock().dac.set_volume(volume).unwrap();
                // Start the timer to hide the bar.
                let state_ = state_.clone();
                timer.start(
                    TimerMode::SingleShot,
                    Duration::from_millis(1000),
                    move || {
                        kvs::keys::VOLUME.set(&volume);
                        Backend::get(&state_.borrow().root.unwrap()).set_volume_visible(false);
                    },
                )
            }
        });

        backend.on_brightness_changed({
            let state_ = state.clone();
            let timer = Timer::default();
            move |value| {
                let brightness = (value as f32) / 100.0;
                Device::lock().set_brightness(brightness);
                // Start the timer to hide the bar.
                let state_ = state_.clone();
                timer.start(
                    TimerMode::SingleShot,
                    Duration::from_millis(1000),
                    move || {
                        kvs::keys::BRIGHTNESS.set(&brightness);
                        Backend::get(&state_.borrow().root.unwrap()).set_brightness_visible(false);
                    },
                )
            }
        });

        backend.on_power_off(|| {
            CoreManager::lock().prepare_for_power_off();
            Device::lock().power_off();
        });

        backend.on_reboot(|| {
            CoreManager::lock().prepare_for_power_off();
            Device::lock().reboot();
        });

        // Utility function: add datetime with delta
        backend.on_datetime_add(settings::settings_datetime_add);
    }
}
