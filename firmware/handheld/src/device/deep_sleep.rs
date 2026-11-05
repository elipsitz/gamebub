use std::time::Duration;

use arrayvec::ArrayVec;
use esp_idf_svc::hal::reset::ResetReason;

use crate::device::board;
use crate::kvs;

/// Time the power button must be held to turn on device.
/// The actual time is a little longer than this, due to the chip
/// having to come up out of deep sleep and initialize.
const POWER_HOLD_DURATION: Duration = Duration::from_millis(900);

/// Pin numbers needed to manage pad holds and isolation for deep sleep.
#[derive(Clone, Debug)]
pub struct SleepConfig {
    pub led_pin: i32,
    pub fpga_power_pin: i32,
    pub dac_reset_pin: i32,
    pub unpowered_pins: ArrayVec<i32, 18>,
}

/// Release all digital and RTC pad holds and isolation latches in hardware registers.
#[inline(always)]
pub fn release_holds() {
    unsafe {
        // Release all individual RTC pad holds
        core::ptr::write_volatile(esp_idf_svc::sys::RTC_CNTL_PAD_HOLD_REG as *mut u32, 0);
        // Release all individual digital pad holds
        core::ptr::write_volatile(esp_idf_svc::sys::RTC_CNTL_DIG_PAD_HOLD_REG as *mut u32, 0);

        // Release global force hold
        let pwc = core::ptr::read_volatile(esp_idf_svc::sys::RTC_CNTL_PWC_REG as *mut u32);
        core::ptr::write_volatile(
            esp_idf_svc::sys::RTC_CNTL_PWC_REG as *mut u32,
            pwc & !(1 << esp_idf_svc::sys::RTC_CNTL_PAD_FORCE_HOLD_S),
        );

        // Clear digital auto-hold and force unhold
        let dig_iso = core::ptr::read_volatile(esp_idf_svc::sys::RTC_CNTL_DIG_ISO_REG as *mut u32);
        core::ptr::write_volatile(
            esp_idf_svc::sys::RTC_CNTL_DIG_ISO_REG as *mut u32,
            (dig_iso & !(1 << esp_idf_svc::sys::RTC_CNTL_DG_PAD_AUTOHOLD_EN_S))
                | (1 << esp_idf_svc::sys::RTC_CNTL_DG_PAD_FORCE_UNHOLD_S)
                | (1 << esp_idf_svc::sys::RTC_CNTL_CLR_DG_PAD_AUTOHOLD_S),
        );
    }
}

/// Configure a pin for digital input with pull-up enabled (safe for wake stub)
#[inline(always)]
unsafe fn wake_stub_init_pin(gpio: i32) {
    let io_mux_reg = (esp_idf_svc::sys::PERIPHS_IO_MUX_GPIO0_U + (gpio as u32) * 4) as *mut u32;
    core::ptr::write_volatile(
        io_mux_reg,
        (1 << esp_idf_svc::sys::MCU_SEL_S)
            | (1 << esp_idf_svc::sys::FUN_PU_S)
            | (1 << esp_idf_svc::sys::FUN_IE_S),
    );

    // If the pin is in the RTC IO range (0..=14 on ESP32-S3), clear MUX_SEL (bit 19)
    // so the pad routes to the digital IO MUX.
    if (0..=14).contains(&gpio) {
        const RTC_IO_TOUCH_PAD0_REG: u32 = esp_idf_svc::sys::DR_REG_RTCIO_BASE + 0x84;
        let rtc_pad_reg = (RTC_IO_TOUCH_PAD0_REG + (gpio as u32) * 4) as *mut u32;
        let rtc_val = core::ptr::read_volatile(rtc_pad_reg);
        core::ptr::write_volatile(rtc_pad_reg, rtc_val & !(1 << 19));
    }
}

/// Read the digital level of a pin (safe for wake stub)
#[inline(always)]
unsafe fn wake_stub_read_pin(gpio: i32) -> u32 {
    let gpio_in = core::ptr::read_volatile(esp_idf_svc::sys::GPIO_IN_REG as *const u32);
    (gpio_in >> (gpio as u32)) & 1
}

/// Deep sleep wake stub called by ROM immediately upon waking from deep sleep.
///
/// The second stage bootloader and application have not run, this cannot rely on anything
/// not in RTC Fast Memory.
#[link_section = ".rtc.text"]
unsafe extern "C" fn wake_stub() {
    esp_idf_svc::sys::esp_default_wake_deep_sleep();

    wake_stub_init_pin(board::PIN_POWER_SWITCH);

    // Verify that the power button is held continuously.
    for _ in 0..(POWER_HOLD_DURATION.as_millis() / 10) {
        esp_idf_svc::sys::esp_rom_delay_us(10_000);
        if wake_stub_read_pin(board::PIN_POWER_SWITCH) != 0 {
            // Button released early: restore RTC function on PIN_POWER_SWITCH
            // and return to deep sleep
            if board::PIN_POWER_SWITCH <= 14 {
                const RTC_IO_TOUCH_PAD0_REG: u32 = esp_idf_svc::sys::DR_REG_RTCIO_BASE + 0x84;
                let rtc_pad =
                    (RTC_IO_TOUCH_PAD0_REG + (board::PIN_POWER_SWITCH as u32) * 4) as *mut u32;
                let rtc_val = core::ptr::read_volatile(rtc_pad);
                core::ptr::write_volatile(rtc_pad, rtc_val | (1 << 19));
            }

            esp_idf_svc::sys::esp_wake_stub_sleep(Some(wake_stub));
            return;
        }
    }

    // Power button was held for the full duration!

    // Invalidate bootloader retain memory so that the bootloader aborts fast-boot.
    // Without this, we cannot enter the DFU app.
    core::ptr::write_volatile(0x600f_fff0 as *mut u32, 0);
    core::ptr::write_volatile(0x600f_fffc as *mut u32, 0);

    // After this, the second-stage bootloader runs.
    release_holds();
}

/// Apply deep sleep pad holds before entering sleep.
fn apply_holds(config: &SleepConfig) {
    unsafe {
        // Register the deep sleep wake stub.
        esp_idf_svc::sys::esp_set_deep_sleep_wake_stub(Some(wake_stub));

        // Drive some pins low, latch them with RTC pad hold.
        for pin in [config.led_pin, config.fpga_power_pin, config.dac_reset_pin] {
            esp_idf_svc::sys::esp_rom_gpio_pad_select_gpio(pin as u32);
            esp_idf_svc::sys::gpio_set_direction(
                pin,
                esp_idf_svc::sys::gpio_mode_t_GPIO_MODE_OUTPUT,
            );
            esp_idf_svc::sys::gpio_set_level(pin, 0);
            esp_idf_svc::sys::gpio_hold_en(pin);
        }

        esp_idf_svc::sys::gpio_pullup_dis(config.led_pin);
        esp_idf_svc::sys::gpio_pulldown_en(config.led_pin);

        esp_idf_svc::sys::gpio_deep_sleep_hold_en();
    }
}

/// High-Z pins connecting to unpowered domains to prevent accidentally
/// backpowering unpowered rails.
fn isolate_pins(config: &SleepConfig) {
    unsafe {
        for &pin in &config.unpowered_pins {
            esp_idf_svc::sys::gpio_set_direction(
                pin,
                esp_idf_svc::sys::gpio_mode_t_GPIO_MODE_DISABLE,
            );
            esp_idf_svc::sys::gpio_pulldown_dis(pin);
            esp_idf_svc::sys::gpio_pullup_dis(pin);
        }
    }
}

/// Enter deep sleep after configuring pads and wakeups.
pub fn enter(config: &SleepConfig) -> ! {
    isolate_pins(config);
    apply_holds(config);

    unsafe {
        esp_idf_svc::sys::esp_sleep_disable_wakeup_source(
            esp_idf_svc::sys::esp_sleep_source_t_ESP_SLEEP_WAKEUP_ALL,
        );
        esp_idf_svc::sys::esp_sleep_enable_ext0_wakeup(board::PIN_POWER_SWITCH, 0);
        esp_idf_svc::sys::esp_deep_sleep_start();
    }
}

/// Check whether deep sleep is used for power off.
fn is_deep_sleep_enabled() -> bool {
    kvs::keys::DEEP_SLEEP.get().unwrap_or(false)
}

/// Called early after power-on to check that we should continue powering on.
pub fn confirm_power_on(board: &board::Board) {
    // If deep sleep is enabled, but this is a cold boot, just enter deep sleep immediately.
    // The wakeup stub will verify that power is properly held down.
    if is_deep_sleep_enabled() && ResetReason::get() == ResetReason::PowerOn {
        enter(&board.sleep_config);
    }

    // Any other boot: release pad holds and proceed.
    release_holds();
}
