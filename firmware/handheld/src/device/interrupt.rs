use std::{num::NonZeroU32, sync::Arc};

use esp_idf_svc::{
    hal::{
        gpio::{InputMode, InterruptType, Pin, PinDriver},
        task::notification::{Notification, Notifier},
    },
    sys::EspError,
};

use crate::worker;
use crate::{device::drivers::fpga, ui};

use super::Device;

const FLAG_MCU_IRQ: NonZeroU32 = NonZeroU32::new(1).unwrap();
const FLAG_HOME: NonZeroU32 = NonZeroU32::new(2).unwrap();
const FLAG_POWER: NonZeroU32 = NonZeroU32::new(4).unwrap();
const FLAG_VOL_UP: NonZeroU32 = NonZeroU32::new(8).unwrap();
const FLAG_VOL_DOWN: NonZeroU32 = NonZeroU32::new(16).unwrap();
const FLAG_VBUS_PGOOD: NonZeroU32 = NonZeroU32::new(32).unwrap();
const FLAG_BUTTONS: u32 =
    FLAG_HOME.get() | FLAG_POWER.get() | FLAG_VOL_UP.get() | FLAG_VOL_DOWN.get();

fn setup_gpio_interrupt(
    pin: &mut PinDriver<'_, impl Pin, impl InputMode>,
    interrupt_type: InterruptType,
    notifier: Arc<Notifier>,
    flags: NonZeroU32,
) -> Result<(), EspError> {
    // SAFETY: only ISR-safe FreeRTOS functions will be called (task notify).
    unsafe {
        pin.subscribe(move || {
            notifier.notify_and_yield(flags);
        })?;
    }
    pin.set_interrupt_type(interrupt_type)?;
    pin.enable_interrupt()?;
    Ok(())
}

impl Device<'_> {
    /// Setup interrupts on the Device interrupt sources:
    ///
    /// * Volume up, volume down, home, and power buttons
    /// * Shared MCU_IRQ line
    pub(super) fn setup_interrupts() {
        // Setup interrupt handler thread.
        std::thread::Builder::new()
            .name("Interrupt".to_string())
            .stack_size(4 * 1024)
            .spawn(|| {
                let notification = Notification::new();

                {
                    let device = &mut Device::get().lock().unwrap();
                    setup_gpio_interrupt(
                        &mut device.button_home,
                        InterruptType::AnyEdge,
                        notification.notifier(),
                        FLAG_HOME,
                    )
                    .unwrap();
                    setup_gpio_interrupt(
                        &mut device.button_power,
                        InterruptType::AnyEdge,
                        notification.notifier(),
                        FLAG_POWER,
                    )
                    .unwrap();
                    setup_gpio_interrupt(
                        &mut device.button_vol_up,
                        InterruptType::AnyEdge,
                        notification.notifier(),
                        FLAG_VOL_UP,
                    )
                    .unwrap();
                    setup_gpio_interrupt(
                        &mut device.button_vol_down,
                        InterruptType::AnyEdge,
                        notification.notifier(),
                        FLAG_VOL_DOWN,
                    )
                    .unwrap();
                    setup_gpio_interrupt(
                        &mut device.pin_irq,
                        InterruptType::LowLevel,
                        notification.notifier(),
                        FLAG_MCU_IRQ,
                    )
                    .unwrap();
                    setup_gpio_interrupt(
                        &mut device.pin_vbus_pgood,
                        InterruptType::AnyEdge,
                        notification.notifier(),
                        FLAG_VBUS_PGOOD,
                    )
                    .unwrap();
                }

                #[allow(unused)]
                let mut prev_hdmi_detected: Option<bool> = None;
                #[allow(unused)]
                let mut prev_vbus_pgood: Option<bool> = None;

                loop {
                    let flags = match notification.wait(esp_idf_svc::hal::delay::BLOCK) {
                        Some(flags) => flags.get(),
                        _ => continue,
                    };

                    let mut device = Device::get().lock().unwrap();

                    if (flags & FLAG_HOME.get()) != 0 {
                        let _ = device.button_home.enable_interrupt();
                    }
                    if (flags & FLAG_POWER.get()) != 0 {
                        let _ = device.button_power.enable_interrupt();
                    }
                    if (flags & FLAG_VOL_UP.get()) != 0 {
                        let _ = device.button_vol_up.enable_interrupt();
                    }
                    if (flags & FLAG_VOL_DOWN.get()) != 0 {
                        let _ = device.button_vol_down.enable_interrupt();
                    }
                    if (flags & FLAG_VBUS_PGOOD.get()) != 0 {
                        let _ = device.pin_vbus_pgood.enable_interrupt();
                    }
                    let mut poll_buttons = (flags & FLAG_BUTTONS) != 0;

                    // Rev 1 and 2, must read I/O expander to clear IRQ.
                    #[cfg(feature = "has_io_expander")]
                    #[allow(unused)]
                    let io_expander = device.io_expander.get_pins().unwrap();

                    // Handle dock monitoring
                    cfg_if::cfg_if! {
                        if #[cfg(feature = "rev1")] {
                            // Docking is based on HDMI hot plug
                            let hdmi_detected = device.parse_hdmi_detect(io_expander).unwrap();
                            if prev_hdmi_detected != Some(hdmi_detected) {
                                prev_hdmi_detected = Some(hdmi_detected);

                                if hdmi_detected {
                                    worker::send(worker::Message::DockBegin { serial: 0, hardware: 0, firmware: 0 });
                                } else {
                                    worker::send(worker::Message::DockEnd);
                                }
                            }
                        } else {
                            // On VBUS pgood falling, force undock
                            let vbus_pgood = device.get_vbus_pgood();
                            if prev_vbus_pgood != Some(vbus_pgood) {
                                prev_vbus_pgood = Some(vbus_pgood);
                                if !vbus_pgood {
                                    worker::send(worker::Message::DockEnd);
                                }
                            }
                        }
                    }

                    if (flags & FLAG_MCU_IRQ.get()) != 0 {
                        log::debug!("Interrupt: MCU_IRQ");

                        // Fuel gauge IRQs.
                        #[cfg(feature = "has_max17048")]
                        if let Ok(fuel_irq) = device.fuel_gauge.query_alerts() {
                            let _ = fuel_irq;
                        }

                        // DAC IRQs.
                        if let Ok(dac_irq) = device.dac.get_interrupt_status() {
                            if dac_irq.headset_detected {
                                let has_headphones = device.dac.get_headphones_detected().unwrap();
                                worker::send(worker::Message::HeadphoneState(has_headphones));
                            }
                        }

                        // FPGA IRQs
                        let fpga_irq = device.fpga.read_u32(fpga::REG_CTRL_IRQ_PENDING).unwrap();
                        if fpga_irq != 0 {
                            device
                                .fpga
                                .write_u32(fpga::REG_CTRL_IRQ_PENDING, fpga_irq)
                                .unwrap();
                            worker::send(worker::Message::FpgaIrq(fpga_irq));
                        }
                        if (fpga_irq & fpga::Irq::Button.as_flag()) != 0 {
                            poll_buttons = true;
                        }

                        let _ = device.pin_irq.enable_interrupt();
                    }

                    if poll_buttons {
                        let input_state = device.get_input_state().unwrap();
                        ui::send(ui::Message::InputState(input_state));
                    }
                }
            })
            .unwrap();
    }
}
