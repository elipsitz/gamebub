use std::{cell::RefCell, collections::VecDeque, path::PathBuf, time::Duration};

use slint::{ComponentHandle, Timer, TimerMode, Weak};
use static_cell::StaticCell;

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

const LEVEL_INCREMENT: i32 = 5;

#[derive(Copy, Clone)]
pub(super) enum LevelSetting {
    Volume,
    Brightness,
}

pub struct UiState {
    root: Weak<MainWindow>,

    level_bar_timer: Timer,

    notification_timer: Timer,
    notification_queue: VecDeque<notifications::Notification>,
    notification_active: bool,

    game_cartridge: bool,
    settings: settings::SettingsState,
    core_file_select_directory: PathBuf,
    core_file_select_filename: String,
    core_file_select_timer: Timer,
}

static UI_STATE: StaticCell<RefCell<UiState>> = StaticCell::new();

impl UiState {
    pub fn new(root: &MainWindow, device: &mut Device) -> &'static RefCell<Self> {
        let state = UI_STATE.init_with(|| {
            RefCell::new(UiState {
                root: root.as_weak(),
                level_bar_timer: Timer::default(),
                notification_timer: Timer::default(),
                notification_queue: VecDeque::new(),
                notification_active: false,
                game_cartridge: false,
                settings: settings::SettingsState::default(),
                core_file_select_directory: PathBuf::new(),
                core_file_select_filename: String::new(),
                core_file_select_timer: Timer::default(),
            })
        });
        state.borrow_mut().setup(state, device);
        state
    }

    pub fn update_battery_level(&mut self, level: f32) {
        let level = level.round() as i32;
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_battery_level(level);
    }

    pub(super) fn adjust_level(&self, setting: LevelSetting, up: bool) {
        let delta = if up {
            LEVEL_INCREMENT
        } else {
            -LEVEL_INCREMENT
        };
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        match setting {
            LevelSetting::Volume => {
                let level = (backend.get_volume_level() + delta).clamp(0, 100);
                backend.set_volume_level(level);
                backend.set_volume_visible(true);
                backend.set_brightness_visible(false);
                let volume = ((level * (u8::MAX as i32) + 50) / 100) as u8;
                Device::lock().dac.set_volume(volume).unwrap();
                kvs::keys::VOLUME.set(&volume);
            }
            LevelSetting::Brightness => {
                let level = (backend.get_brightness_level() + delta).clamp(0, 100);
                backend.set_brightness_level(level);
                backend.set_brightness_visible(true);
                backend.set_volume_visible(false);
                let brightness = (level as f32) / 100.0;
                Device::lock().set_brightness(brightness);
                kvs::keys::BRIGHTNESS.set(&brightness);
            }
        }

        let weak = self.root.clone();
        self.level_bar_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(1000),
            move || {
                if let Some(root) = weak.upgrade() {
                    let backend = root.global::<Backend>();
                    backend.set_volume_visible(false);
                    backend.set_brightness_visible(false);
                }
            },
        );
    }

    fn setup(&mut self, state: &'static RefCell<UiState>, device: &mut Device) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        let battery_level = device.fuel_gauge.get_battery_level().unwrap_or(0.0);
        self.update_battery_level(battery_level);
        backend.set_volume_level(((kvs::keys::VOLUME.get().unwrap() as i32) * 100 + 127) / 255);
        backend.set_brightness_level((kvs::keys::BRIGHTNESS.get().unwrap() * 100.0).round() as i32);
        backend.set_hardware_version(crate::hwinfo::get_hardware_version().to_string().into());
        backend.set_hardware_serial(crate::hwinfo::get_serial_number().to_string().into());
        backend.set_firmware_version(env!("CARGO_PKG_VERSION").into());
        backend.set_sdcard_mounted(std::fs::exists("/sdcard").unwrap_or(false));

        self.setup_setup(state, device);
        self.setup_main_menu(state, device);
        self.setup_game(state, device);
        self.setup_tools(state, device);
        self.setup_settings(state, device);
        self.setup_cores(state, device);

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
                    self.queue_notification(state, notification);
                }
            }
        }

        backend.on_screen_enter(move |screen| {
            // Called when a new screen is entered, before the new frame is rendered.
            log::info!("Screen enter: {:?}", screen);
            if screen == ScreenId::Settings {
                state.borrow_mut().on_settings_enter()
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
