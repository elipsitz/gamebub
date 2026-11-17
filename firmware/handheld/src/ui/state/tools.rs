use std::cell::RefCell;

use super::super::slint::{Backend, BatteryInfo};
use slint::ComponentHandle;

use crate::device::{drivers::usb, Device};

use super::UiState;

impl UiState {
    /// Set up the "Tools" screen.
    pub(super) fn setup_tools(&mut self, _state: &'static RefCell<UiState>, _device: &mut Device) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        backend.on_tools_start_usb_drive(move || {
            usb::configure_usb(usb::UsbMode::ConsoleAndMassStorage).expect("USB setup failed");
        });
        backend.on_tools_end_usb_drive(move || {
            usb::configure_usb(usb::UsbMode::ConsoleOnly).expect("USB teardown failed");
            Device::lock().reboot();
        });

        backend.on_tools_start_cart_reader(move || {
            usb::configure_usb(usb::UsbMode::ConsoleAndSerial).expect("USB setup failed");
            crate::cart_backup::start_task(1);
        });
        backend.on_tools_end_cart_reader(move || {
            usb::configure_usb(usb::UsbMode::ConsoleOnly).expect("USB teardown failed");
            Device::lock().reboot();
        });

        backend.on_tools_get_battery_info(move || {
            let mut device = Device::lock();
            BatteryInfo {
                is_charging: device.get_battery_is_charging(),
                level: device.fuel_gauge.get_battery_level().unwrap_or(f32::NAN),
                vbus_pgood: device.get_vbus_pgood(),
                voltage: device.fuel_gauge.get_battery_voltage().unwrap_or(f32::NAN),
                current: device.fuel_gauge.get_battery_current().unwrap_or(f32::NAN),
            }
        })
    }
}
