use anyhow::Context;

use device::Device;

use crate::{
    device::drivers::{fpga, usb},
    ui::UI,
};

mod bitstream;
mod cart_backup;
mod control;
mod core;
mod crash_handler;
mod device;
mod fwinfo;
mod hwinfo;
mod input;
mod kvs;
mod led;
mod power;
pub mod ui;
mod util;
mod worker;

enum StartupAction {
    MainMenu,
    RunCartridge,
}

fn get_startup_action() -> StartupAction {
    match kvs::keys::STARTUP_ACTION.get() {
        Some(1) => StartupAction::RunCartridge,
        _ => StartupAction::MainMenu,
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    esp_idf_svc::log::set_target_level("gpio", log::LevelFilter::Warn).unwrap();
    crash_handler::setup();

    if let Err(e) = usb::configure_usb(usb::UsbMode::ConsoleAndVendor) {
        log::error!("USB setup failed: {:?}", e);
    }

    log::info!("Hardware version: {}", hwinfo::get_hardware_version());
    log::info!("Serial: {}", hwinfo::get_serial_number());
    kvs::Kvs::init()?;

    // Check that the firmware is compatible with the listed revision.
    cfg_if::cfg_if! {
        if #[cfg(feature = "rev1")] {
            let required_revision = 1;
        } else if #[cfg(feature = "rev2")] {
            let required_revision = 2;
        } else if #[cfg(feature = "rev3")] {
            let required_revision = 3;
        } else if #[cfg(feature = "rev4")] {
            let required_revision = 4;
        } else {
            compile_error!("No board revision selected");
        }
    };
    let actual_revision = hwinfo::get_hardware_version().major;
    if actual_revision != required_revision && actual_revision != 0 && actual_revision != 255 {
        anyhow::bail!(
            "Incompatible firmware revision: device={:?} firmware={}",
            actual_revision,
            required_revision
        );
    }

    // Proceed to initialize device.
    Device::init()?;
    let mut device = Device::lock();
    if device.sdcard.is_none() {
        log::warn!("Failed to mount SD card");
    }
    device.set_brightness(kvs::keys::BRIGHTNESS.get().unwrap());

    // Setup workers.
    worker::start();
    power::PowerManager::start(&mut device);

    // Initial programming FPGA
    fn program_fpga(device: &mut Device) -> anyhow::Result<()> {
        bitstream::initial_program_boot(device)?;
        device.fpga.enable_interrupt(fpga::Irq::Button)?;
        device.lcd.enable_fpga_control()?;
        Ok(())
    }

    match get_startup_action() {
        StartupAction::RunCartridge => worker::send(worker::Message::RunCartridge),
        StartupAction::MainMenu => program_fpga(&mut device).context("Failed to program FPGA")?,
    }

    // Setup UI
    let mut ui = UI::new(&mut device);
    std::mem::drop(device);

    // Run UI in this thread.
    if let StartupAction::MainMenu = get_startup_action() {
        led::LedController::set_behavior(led::LedBehavior::OFF);
    }
    ui.run();
}
