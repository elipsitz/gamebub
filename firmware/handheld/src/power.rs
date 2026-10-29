use std::{
    sync::{LazyLock, Mutex, MutexGuard},
    time::Duration,
};

use esp_idf_svc::timer::{EspTaskTimerService, EspTimer};

use crate::{
    device::Device,
    led::{self, LedBehavior},
    ui,
};

static POWER_MANAGER: LazyLock<Mutex<PowerManager>> =
    LazyLock::new(|| Mutex::new(PowerManager::new()));

const MONITOR_INTERVAL: Duration = Duration::from_secs(30);
const CUTOFF_VOLTAGE: f32 = 3.3;

static ALERT_LEVELS: [i32; 3] = [20, 10, 5];
const ALERT_HYSTERESIS: i32 = 3;

pub struct PowerManager {
    monitor_timer: Option<EspTimer<'static>>,
    alerts: [bool; ALERT_LEVELS.len()],
}

impl PowerManager {
    fn new() -> Self {
        PowerManager {
            monitor_timer: None,
            alerts: [true; ALERT_LEVELS.len()],
        }
    }

    pub fn lock() -> MutexGuard<'static, Self> {
        POWER_MANAGER.lock().unwrap()
    }

    fn init(&mut self, device: &mut Device) {
        let battery_level = device.fuel_gauge.get_battery_level().ok();
        ui::send(ui::Message::BatteryStatus {
            level: battery_level.unwrap_or(0.),
        });
    }

    #[allow(clippy::needless_range_loop)]
    fn update(&mut self) {
        let mut device = Device::lock();
        let battery_level = device.fuel_gauge.get_battery_level().ok();
        let vbus = device.get_vbus_pgood();
        let battery_voltage = device.fuel_gauge.get_battery_voltage().ok();
        drop(device);

        if let Some(battery_level) = battery_level {
            let battery_level = battery_level.round() as i32;
            let mut fire = false;
            for i in 0..ALERT_LEVELS.len() {
                if self.alerts[i] && battery_level <= ALERT_LEVELS[i] {
                    self.alerts[i] = false;
                    fire = true;
                }
                if battery_level >= ALERT_LEVELS[i] + ALERT_HYSTERESIS {
                    self.alerts[i] = true;
                }
            }
            if fire {
                ui::send(ui::Message::Notification(ui::Notification::new_alert(
                    "Low battery".into(),
                )));
            }
        }

        if let Some(battery_voltage) = battery_voltage {
            if battery_voltage <= CUTOFF_VOLTAGE && !vbus {
                log::warn!("Battery critically low, powering off");
                ui::send(ui::Message::Notification(ui::Notification::new_alert(
                    "Critically low battery!\nShutting down...".into(),
                )));

                std::thread::sleep(Duration::from_secs(3));

                crate::core::CoreManager::lock().prepare_for_power_off();
                led::LedController::set_behavior(LedBehavior::BATTERY_CRITICAL);
                Device::lock().power_off();
            }
        }

        ui::send(ui::Message::BatteryStatus {
            level: battery_level.unwrap_or(0.),
        });
    }

    /// Start periodically polling power state.
    pub fn start(device: &mut Device) {
        let mut manager = PowerManager::lock();
        assert!(manager.monitor_timer.is_none());
        let timer_service = EspTaskTimerService::new().unwrap();
        let timer = timer_service
            .timer(move || {
                PowerManager::lock().update();
            })
            .unwrap();
        timer.every(MONITOR_INTERVAL).unwrap();
        manager.monitor_timer = Some(timer);
        manager.init(device);
    }
}
