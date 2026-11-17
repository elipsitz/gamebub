use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::kvs;
use anyhow::Context;
use drivers::lcd_backlight;
use drivers::sdcard::Sdcard;
use embedded_hal_bus::i2c::MutexDevice as MutexI2C;
use esp_idf_svc::hal::gpio::{
    self, AnyIOPin, AnyInputPin, DriveStrength, Input, InputOutput, Level,
};
use esp_idf_svc::hal::gpio::{AnyOutputPin, Output, PinDriver};
use esp_idf_svc::hal::ledc::{LedcDriver, LedcTimerDriver};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::spi::{
    self, SpiDeviceDriver, SpiDriver, SpiDriverConfig, SpiSharedDeviceDriver, SpiSoftCsDeviceDriver,
};
use esp_idf_svc::hal::units::{FromValueType, Hertz};
use esp_idf_svc::hal::{i2c::*, ledc};
use static_cell::StaticCell;

pub mod board;
pub mod deep_sleep;
pub mod drivers;
mod input;
mod interrupt;
mod led;

/// Time it may take for FPGA power rails to stabilize after enable.
const FPGA_POWER_DELAY: Duration = Duration::from_millis(5);

static DEVICE: OnceLock<Mutex<Device>> = OnceLock::new();
static I2C_DRIVER: StaticCell<Mutex<I2cDriver<'static>>> = StaticCell::new();
static FPGA_SPI_DRIVER: StaticCell<SpiDriver<'static>> = StaticCell::new();
#[cfg(any(feature = "rev2", feature = "rev4"))]
static LCD_SPI_DRIVER: StaticCell<SpiDriver<'static>> = StaticCell::new();

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DisplayMode {
    None,
    Internal,
    External,
}

/// Main container for device hardware.
pub struct Device<'a> {
    /// FPGA power in, active-high.
    fpga_power: PinDriver<'a, AnyOutputPin, Output>,

    /// The I2C bus.
    #[allow(unused)]
    i2c: &'a Mutex<I2cDriver<'a>>,

    /// LCD backlight PWM driver
    lcd_backlight: lcd_backlight::PwmBacklight<'a>,

    /// LCD driver
    #[cfg(feature = "has_ili9488")]
    pub lcd: drivers::ili9488::ILI9488<
        PinDriver<'a, AnyOutputPin, Output>,
        PinDriver<'a, AnyOutputPin, Output>,
        SpiSoftCsDeviceDriver<'a, SpiSharedDeviceDriver<'a, &'a SpiDriver<'a>>, &'a SpiDriver<'a>>,
    >,
    #[cfg(feature = "has_st7262")]
    pub lcd: drivers::st7262::ST7262<PinDriver<'a, AnyOutputPin, Output>>,
    #[cfg(feature = "has_ili9806e")]
    pub lcd: drivers::ili9806e::ILI9806E<
        PinDriver<'a, AnyOutputPin, Output>,
        SpiSoftCsDeviceDriver<'a, SpiSharedDeviceDriver<'a, &'a SpiDriver<'a>>, &'a SpiDriver<'a>>,
    >,

    /// Display mode (if initialized)
    pub display_mode: DisplayMode,

    /// DAC driver
    pub dac: drivers::dac::TLV320DAC3101<
        PinDriver<'a, AnyOutputPin, Output>,
        MutexI2C<'a, I2cDriver<'a>>,
    >,

    /// FPGA driver
    #[allow(clippy::type_complexity)]
    pub fpga: drivers::fpga::Fpga<
        'a,
        PinDriver<'a, AnyInputPin, Input>,
        PinDriver<'a, AnyOutputPin, Output>,
        PinDriver<'a, AnyIOPin, Input>,
        SpiDeviceDriver<'a, &'a SpiDriver<'a>>,
    >,

    /// RTC driver
    pub rtc: drivers::rtc::PCF8563<MutexI2C<'a, I2cDriver<'a>>>,

    /// Battery fuel gauge driver
    #[cfg(feature = "has_max17048")]
    pub fuel_gauge: drivers::max17048::MAX17048<MutexI2C<'a, I2cDriver<'a>>>,
    #[cfg(feature = "has_bq27427")]
    pub fuel_gauge: drivers::bq27427::BQ27427<MutexI2C<'a, I2cDriver<'a>>>,

    /// IMU driver
    pub imu: drivers::imu::LSM6DS3TRC<MutexI2C<'a, I2cDriver<'a>>>,

    #[cfg(feature = "has_io_expander")]
    io_expander: drivers::io_expander::TCA9535<MutexI2C<'a, I2cDriver<'a>>>,

    button_home: PinDriver<'a, AnyIOPin, Input>,
    button_vol_up: PinDriver<'a, AnyIOPin, Input>,
    button_vol_down: PinDriver<'a, AnyIOPin, Input>,
    button_power: PinDriver<'a, AnyIOPin, InputOutput>,
    pin_irq: PinDriver<'a, AnyInputPin, Input>,
    pin_vbus_pgood: PinDriver<'a, AnyIOPin, Input>,
    pin_batt_chg: PinDriver<'a, AnyInputPin, Input>,

    pin_cart_power: Option<PinDriver<'a, AnyOutputPin, Output>>,
    pin_cart_switch: Option<PinDriver<'a, AnyInputPin, Input>>,

    /// Sdcard,
    pub sdcard: Option<Sdcard>,

    /// Dock state
    pub docked: bool,

    /// Deep sleep board configuration
    pub sleep_config: deep_sleep::SleepConfig,
}

impl Device<'_> {
    pub fn init(board: board::Board) -> Result<(), anyhow::Error> {
        if DEVICE.get().is_some() {
            panic!("Device already initialized.");
        }

        let sleep_config = board.sleep_config.clone();
        let sd_pins = board.pins.sd_pins();

        // Status LED: active high
        let led = LedcDriver::new(
            board.ledc.channel0,
            LedcTimerDriver::new(
                board.ledc.timer0,
                &ledc::config::TimerConfig::new()
                    .frequency(1.kHz().into())
                    .resolution(ledc::config::Resolution::Bits14),
            )?,
            board.pins.led,
        )
        .context("status led")?;
        crate::led::LedController::start(led);
        crate::led::LedController::set_behavior(crate::led::LedBehavior::LOADING);

        // Mount sdcard to /sdcard
        let sdcard = drivers::sdcard::mount_sdcard(&sd_pins).ok();
        if sdcard.is_some() {
            crate::crash_handler::maybe_persist();
        }

        // TODO: see if we can avoid keeping FPGA power on all the time
        let mut fpga_power = PinDriver::output(board.pins.fpga_power)?;
        fpga_power.set_high()?;
        let fpga_power_time = Instant::now();

        // Initialize I2C
        let i2c_config = I2cConfig::new()
            .baudrate(400.kHz().into())
            .timeout(Duration::from_millis(10).into());
        let i2c = I2cDriver::new(
            board.i2c,
            board.pins.i2c_sda,
            board.pins.i2c_scl,
            &i2c_config,
        )
        .context("I2cDriver")?;
        let i2c = &*I2C_DRIVER.init(Mutex::new(i2c));

        let pin_irq = PinDriver::input(board.pins.irq)?;

        // LCD backlight
        let lcd_backlight = {
            #[cfg(any(feature = "rev1", feature = "rev2"))]
            let config = lcd_backlight::PwmConfig {
                frequency: 25.kHz().into(),
                resolution: ledc::config::Resolution::Bits11,
                min_duty: 0.01,
                gamma: 2.2,
            };
            #[cfg(feature = "rev3")]
            let config = lcd_backlight::PwmConfig {
                frequency: 256.Hz(),
                resolution: ledc::config::Resolution::Bits14,
                min_duty: 0.03,
                gamma: 2.8,
            };
            #[cfg(feature = "rev4")]
            let config = lcd_backlight::PwmConfig {
                frequency: 20.kHz().into(),
                resolution: ledc::config::Resolution::Bits11,
                min_duty: 0.001,
                gamma: 1.8,
            };
            let driver = LedcDriver::new(
                board.ledc.channel1,
                LedcTimerDriver::new(
                    board.ledc.timer1,
                    &ledc::config::TimerConfig::new()
                        .frequency(config.frequency)
                        .resolution(config.resolution),
                )?,
                board.pins.lcd_backlight,
            )
            .context("backlight led")?;
            let mut backlight = lcd_backlight::PwmBacklight::new(config, driver);
            backlight.init();
            backlight
        };

        // Setup SPI
        // Use DMA transfers, with an auto-assigned channel, and a maximum transfer size of 32 KiB.
        let spi_driver_config = SpiDriverConfig::new().dma(spi::Dma::Auto(32 * 1024));
        let fpga_spi_driver = SpiDriver::new_quad(
            board.spi2,
            board.pins.fpga_spi_clk,
            board.pins.fpga_spi_d0,
            board.pins.fpga_spi_d1,
            board.pins.fpga_spi_d2,
            board.pins.fpga_spi_d3,
            &spi_driver_config,
        )
        .context("fpga spi driver")?;
        let fpga_spi_driver = &*FPGA_SPI_DRIVER.init(fpga_spi_driver);
        cfg_if::cfg_if! {
            if #[cfg(feature = "rev1")] {
                let lcd_spi_driver = fpga_spi_driver;
            } else if #[cfg(any(feature = "rev2", feature = "rev4"))] {
                let lcd_spi_driver = SpiDriver::new(
                    board.spi3,
                    board.pins.lcd_spi_clk.expect("lcd_spi_clk"),
                    board.pins.lcd_spi_d0.expect("lcd_spi_d0"),
                    Option::<AnyInputPin>::None,
                    &spi_driver_config,
                )
                .context("lcd spi driver")?;
                let lcd_spi_driver = &*LCD_SPI_DRIVER.init(lcd_spi_driver);
            }
        }

        // Setup LCD
        cfg_if::cfg_if! {
            if #[cfg(feature = "has_ili9488")] {
                log::info!("Initializing LCD");
                let lcd_spi_config = spi::config::Config::new().baudrate(10.MHz().into());
                let lcd_spi = SpiSoftCsDeviceDriver::new(
                    SpiSharedDeviceDriver::new(lcd_spi_driver, &lcd_spi_config)?,
                    board.pins.lcd_cs.expect("lcd_cs"),
                    gpio::Level::High,
                )?;
                let lcd_reset = PinDriver::output(board.pins.lcd_reset.expect("lcd_reset"))?;
                let lcd_dc = PinDriver::output(board.pins.lcd_dc.expect("lcd_dc"))?;
                let mut lcd = drivers::ili9488::ILI9488::new(lcd_reset, lcd_dc, lcd_spi);
            } else if #[cfg(feature = "has_st7262")] {
                let lcd_enable = PinDriver::output(board.pins.lcd_enable.expect("lcd_enable"))?;
                let mut lcd = drivers::st7262::ST7262::new(lcd_enable);
            } else if #[cfg(feature = "has_ili9806e")] {
                log::info!("Initializing LCD");
                let lcd_spi_config = spi::config::Config::new().baudrate(5.MHz().into());
                let lcd_spi = SpiSoftCsDeviceDriver::new(
                    SpiSharedDeviceDriver::new(lcd_spi_driver, &lcd_spi_config)?,
                    board.pins.lcd_cs.expect("lcd_cs"),
                    gpio::Level::High,
                )?;
                let lcd_reset = PinDriver::output(board.pins.lcd_reset.expect("lcd_reset"))?;
                let mut lcd = drivers::ili9806e::ILI9806E::new(lcd_reset, lcd_spi);
            }
        }
        lcd.init().context("LCD init")?;

        // Setup I/O expander (Rev 1 and Rev 2 only)
        cfg_if::cfg_if! {
            if #[cfg(feature = "has_io_expander")] {
                let mut io_expander = drivers::io_expander::TCA9535::new(MutexI2C::new(&i2c));
                io_expander.get_pins()?;
            }
        }

        // Direct buttons
        let mut button_home = PinDriver::input(board.pins.home)?;
        let mut button_vol_up = PinDriver::input(board.pins.vol_up)?;
        let mut button_vol_down = PinDriver::input(board.pins.vol_down)?;
        let mut button_power = PinDriver::input_output_od(board.pins.power_switch)?;
        button_home.set_pull(gpio::Pull::Up)?;
        button_vol_up.set_pull(gpio::Pull::Up)?;
        button_vol_down.set_pull(gpio::Pull::Up)?;
        button_power.set_high()?;

        // Cartridge switch and power
        let pin_cart_switch = board
            .pins
            .cart_switch
            .map(|p: AnyInputPin| PinDriver::input(p))
            .transpose()?;
        let pin_cart_power = board
            .pins
            .cart_power
            .map(|p: AnyOutputPin| PinDriver::output(p))
            .transpose()?;

        // Setup RTC
        let rtc = drivers::rtc::PCF8563::new(MutexI2C::new(i2c));

        // Setup battery fuel gauge
        cfg_if::cfg_if! {
            if #[cfg(feature = "has_max17048")] {
                let mut fuel_gauge = drivers::max17048::MAX17048::new(MutexI2C::new(&i2c));
                let _ = fuel_gauge.set_alert_soc_change(true); // fuel gauge won't work without a battery
            } else if #[cfg(feature = "has_bq27427")] {
                let fuel_gauge = drivers::bq27427::BQ27427::new(MutexI2C::new(i2c));

                // Fuel gauge requires configuration, which could block for multiple seconds. Run it in another thread,
                // with a new instance of the driver. Scary, but fine, because the driver has no state
                // and the I2C bus is protected by a mutex.
                // This would be easier if everything were async, most of the blocking is just waiting to poll...
                let i2c_2 = MutexI2C::new(i2c);
                std::thread::Builder::new()
                    .name("battery_setup".to_string())
                    .stack_size(2 * 1024)
                    .spawn(move || {
                        let mut fuel_gauge = drivers::bq27427::BQ27427::new(i2c_2);
                        // TODO: after software update, force reconfigure?
                        let force_configure = false;
                        if fuel_gauge.configure(force_configure).is_err() {
                            log::warn!("Failed to configure fuel gauge");
                        }
                    })
                    .unwrap();
            }
        }

        // Battery charger
        let mut pin_vbus_pgood = PinDriver::input(board.pins.vbus_pgood)?;
        pin_vbus_pgood.set_pull(gpio::Pull::Up)?;
        let pin_batt_chg = PinDriver::input(board.pins.batt_chg)?;

        // Setup IMU
        let mut imu = drivers::imu::LSM6DS3TRC::new(MutexI2C::new(i2c));
        imu.init().context("IMU init")?;

        // Ensure fpga power has stabilized.
        let time_since_fpga_power = Instant::now().duration_since(fpga_power_time);
        std::thread::sleep(FPGA_POWER_DELAY.saturating_sub(time_since_fpga_power));

        // Setup DAC (requires fpga_power on)
        log::info!("Initializing DAC");
        let dac_reset = PinDriver::output(board.pins.dac_reset)?;
        let mut dac = drivers::dac::TLV320DAC3101::new(dac_reset, MutexI2C::new(i2c));
        dac.init().context("DAC init")?;
        dac.configure_interrupts().context("DAC interrupts")?;
        dac.set_power_down(0, true).context("DAC power down")?;
        dac.set_volume(kvs::keys::VOLUME.get().unwrap())
            .context("DAC set volume")?;
        dac.set_mute(false).context("DAC set mute")?;
        let headphones_detected = dac
            .get_headphones_detected()
            .context("DAC get headphones")?;
        dac.set_headphones_enabled(headphones_detected)?;
        dac.set_speakers_enabled(!headphones_detected)?;

        // Setup FPGA (without programming)
        let fpga_done = PinDriver::input(board.pins.fpga_done)?;
        let mut fpga_program_b = PinDriver::output_od(board.pins.fpga_program_b)?;
        fpga_program_b.set_high()?; // Initializing pin sets this to low -- release it to high-z immediately.
        let fpga_init_b = PinDriver::input(board.pins.fpga_init_b)?;

        let spi_rates = [40.MHz(), 20.MHz(), 10.MHz(), 4.MHz(), 2.MHz()];
        let mut pin_fpga_spi_cs = board.pins.fpga_spi_cs;
        let fpga_data_spis = spi_rates
            .iter()
            .map(|&rate| {
                let config = spi::config::Config::new()
                    .baudrate(rate.into())
                    .duplex(spi::config::Duplex::Half);
                // SAFETY: this CS pin is used in all the data SPIs.
                // They're used in the same mode (output), and controlled only by software,
                // and never used at the same time (either read *or* write are used at any time).
                // There doesn't appear to be any other way to use multiple clock speeds.
                let cs_pin = unsafe { pin_fpga_spi_cs.clone_unchecked() };
                let spi = SpiSoftCsDeviceDriver::new(
                    SpiSharedDeviceDriver::new(fpga_spi_driver, &config).unwrap(),
                    cs_pin,
                    gpio::Level::High,
                )
                .unwrap();
                (spi, Hertz::from(rate))
            })
            .collect::<Vec<_>>();
        let fpga_program_config = spi::config::Config::new().baudrate(80.MHz().into());
        let fpga_program_spi = SpiDeviceDriver::new(
            fpga_spi_driver,
            Option::<AnyOutputPin>::None,
            &fpga_program_config,
        )?;
        let fpga = drivers::fpga::Fpga::new(
            fpga_done,
            fpga_program_b,
            fpga_init_b,
            fpga_data_spis,
            fpga_program_spi,
        );

        // Mount system_data to /system
        drivers::fs::mount_system_data().context("mount system data")?;

        let mut device = Device {
            fpga_power,
            i2c,
            lcd_backlight,
            lcd,
            display_mode: DisplayMode::None,
            dac,
            fpga,
            fuel_gauge,
            #[cfg(feature = "has_io_expander")]
            io_expander,
            button_home,
            button_power,
            button_vol_up,
            button_vol_down,
            pin_irq,
            pin_batt_chg,
            pin_vbus_pgood,
            pin_cart_power,
            pin_cart_switch,
            rtc,
            imu,
            sdcard,
            docked: false,
            sleep_config,
        };
        device.init_datetime();
        DEVICE
            .set(Mutex::new(device))
            .map_err(|_| ())
            .expect("Device already initialized");

        Device::setup_interrupts();

        Ok(())
    }

    pub fn get() -> &'static Mutex<Device<'static>> {
        DEVICE.get().unwrap()
    }

    pub fn lock() -> MutexGuard<'static, Device<'static>> {
        Device::get().lock().unwrap()
    }

    /// Called early after power-on to check that we should continue powering on.
    pub fn confirm_power_on(board: &board::Board) {
        deep_sleep::confirm_power_on(board);
    }

    /// Enable or disable FPGA power.
    ///
    /// Note that it may take around 5ms to stabilize.
    pub fn set_fpga_power(&mut self, enable: bool) -> Result<(), anyhow::Error> {
        self.fpga_power.set_level(enable.into())?;
        Ok(())
    }

    /// Display a framebuffer.
    ///
    /// Currently always an FPGA overlay.
    pub fn display_framebuffer_raw(&mut self, raw: &[u8]) {
        let _ = self.fpga.write_overlay(0, raw);
    }

    /// Gracefully turn the device off.
    pub fn power_off(&mut self) -> ! {
        log::info!("Powering off");

        self.prepare_for_power_off();

        if kvs::keys::DEEP_SLEEP.get().unwrap_or(false) {
            // Prepare for extended deep sleep.
            deep_sleep::enter(&self.sleep_config);
        } else {
            // Hold power button to power-off.

            // Leave FPGA powered, but put it in program-reset mode.
            // A steady load (FPGA quiescent current) helps to drain capacitors.
            let _ = self.set_fpga_power(true);
            let _ = self.fpga.pin_program_b.set_low();

            // Hold down the power button until the device shuts off.
            let _ = self.fpga_power.set_drive_strength(DriveStrength::I40mA);
            let _ = self.button_power.set_drive_strength(DriveStrength::I40mA);
            let _ = self.button_power.set_low();

            unsafe {
                // Enable pin hold during deep sleep.
                esp_idf_svc::sys::gpio_hold_en(self.fpga_power.pin());
                esp_idf_svc::sys::gpio_hold_en(self.fpga.pin_program_b.pin());
                esp_idf_svc::sys::gpio_hold_en(self.button_power.pin());
                esp_idf_svc::sys::gpio_deep_sleep_hold_en();

                // Stay in deep sleep to maximize chances of holding power button down past brown-out.
                esp_idf_svc::sys::esp_sleep_disable_wakeup_source(
                    esp_idf_svc::sys::esp_sleep_source_t_ESP_SLEEP_WAKEUP_ALL,
                );
            }
        }

        unsafe {
            // Enter deep sleep.
            esp_idf_svc::sys::esp_deep_sleep_start();
        }
    }

    /// Gracefully soft reset.
    pub fn reboot(&mut self) -> ! {
        log::info!("Rebooting");
        self.prepare_for_power_off();
        esp_idf_svc::hal::reset::restart();
    }

    /// Gracefully reset into user DFU mode.
    pub fn reboot_dfu(&mut self) -> ! {
        const DFU_HINT: u32 = 0x1B01;
        log::info!("Preparing for DFU");
        unsafe { esp_idf_svc::sys::esp_reset_reason_set_hint(DFU_HINT) };
        self.reboot()
    }

    /// Prepare for power-off or reset, by powering down peripherals and saving state.
    ///
    /// The device must go through a [soft] reset to function correctly after this.
    fn prepare_for_power_off(&mut self) {
        // Unmount SD card if mounted to flush writes and deinit
        if let Some(sdcard) = self.sdcard.take() {
            let _ = sdcard.unmount();
        }

        let _ = self.change_display_mode(DisplayMode::None);

        // Turn off status LED
        crate::led::LedController::set_behavior(crate::led::LedBehavior::OFF);

        // Power down DAC and hold in reset
        self.dac.deinitialize();

        // Power down IMU
        let _ = self.imu.disable_accel();
        let _ = self.imu.disable_gyro();

        self.set_lcd_enabled(false);

        let _ = self.set_fpga_power(false);
        self.set_cart_power(false);

        // Update uptime
        let this_uptime =
            Duration::from_micros(unsafe { esp_idf_svc::sys::esp_timer_get_time() as _ });
        let new_uptime = kvs::keys::UPTIME.get().unwrap() + (this_uptime.as_secs() as u32);
        kvs::keys::UPTIME.set(&new_uptime);

        kvs::keys::flush_all();
    }

    /// Set whether the LCD is enabled or disabled.
    pub fn set_lcd_enabled(&mut self, enabled: bool) {
        self.lcd_backlight.set_enabled(enabled);
        if enabled {
            let _ = self.lcd.exit_sleep();
        } else {
            let _ = self.lcd.enter_sleep();
        }
    }

    /// Set the LCD brightness. The input is a float in the range [0.0, 1.0].
    pub fn set_brightness(&mut self, brightness: f32) {
        self.lcd_backlight.set_brightness(brightness);
    }

    /// Initialize the system time (after boot).
    ///
    /// Reads the time from the RTC. Sets a default time if no time is set.
    /// Then sets it in esp-idf (via libc settimeofday).
    fn init_datetime(&mut self) {
        if self.rtc.read_datetime().unwrap().is_none() {
            log::warn!("No date set, resetting");
            self.rtc
                .write_datetime(drivers::rtc::Datetime::default())
                .unwrap();
        }
        Device::set_esp_datetime(self.get_datetime());
    }

    /// Set the esp-idf system time.
    fn set_esp_datetime(dt: time::OffsetDateTime) {
        let timeval = esp_idf_svc::sys::timeval {
            tv_sec: dt.unix_timestamp(),
            tv_usec: 0,
        };
        unsafe {
            // SAFETY: this is safe to call with valid or null pointers.
            esp_idf_svc::sys::settimeofday(&timeval, std::ptr::null());
        }
    }

    /// Get the Device datetime.
    pub fn get_datetime(&mut self) -> time::OffsetDateTime {
        let rtc_time = self.rtc.read_datetime().unwrap();
        let ts = rtc_time
            .and_then(|dt| dt.as_timestamp())
            .unwrap_or(drivers::rtc::TIMESTAMP_2000);
        time::OffsetDateTime::from_unix_timestamp(ts as i64).unwrap()
    }

    /// Set the Device datetime.
    pub fn set_datetime(&mut self, dt: time::OffsetDateTime) {
        log::info!("Setting system time: {:?}", dt);
        Device::set_esp_datetime(dt);
        let ts = dt.unix_timestamp();
        let dt = drivers::rtc::Datetime::from_timestamp(ts as u64).unwrap_or_default();
        self.rtc.write_datetime(dt).unwrap();
    }

    /// Update the physical dock state and refresh display mode.
    pub fn set_docked(&mut self, docked: bool) -> Result<(), anyhow::Error> {
        self.docked = docked;
        self.refresh_display_mode()
    }

    /// Refresh display mode based on dock state and settings.
    pub fn refresh_display_mode(&mut self) -> Result<(), anyhow::Error> {
        let target_mode = if self.docked
            && !crate::kvs::keys::DOCK_USE_BUILTIN_SCREEN
                .get()
                .unwrap_or(false)
        {
            DisplayMode::External
        } else {
            DisplayMode::Internal
        };
        self.change_display_mode(target_mode)
    }

    /// Apply the current dock state and display mode to the FPGA dock register.
    pub fn apply_dock_state(&mut self) -> Result<(), anyhow::Error> {
        let use_builtin = self.docked && self.display_mode == DisplayMode::Internal;
        self.fpga
            .set_dock_state(self.docked, use_builtin)
            .context("set FPGA dock state")?;
        Ok(())
    }

    /// Update the display mode (internal vs external).
    pub fn change_display_mode(&mut self, new_mode: DisplayMode) -> Result<(), anyhow::Error> {
        let old_mode = self.display_mode;
        if old_mode == new_mode {
            self.apply_dock_state()?;
            return Ok(());
        }
        log::info!("Display mode: {:?}", new_mode);

        if old_mode == DisplayMode::Internal {
            self.dac.set_power_down(1, true)?;
            self.lcd_backlight.set_enabled(false);
            self.lcd.enter_sleep()?;
        }

        self.display_mode = new_mode;
        self.apply_dock_state()?;

        if new_mode == DisplayMode::Internal {
            self.lcd.exit_sleep().context("LCD exit sleep")?;

            // Let LCD stabilize and refresh before turning on backlight. Measured empirically.
            std::thread::sleep(Duration::from_millis(200));
            self.lcd_backlight.set_enabled(true);
            self.dac.set_power_down(1, false)?;
        }

        Ok(())
    }

    pub fn get_display_mode(&self) -> DisplayMode {
        self.display_mode
    }

    pub fn get_battery_is_charging(&self) -> bool {
        self.pin_batt_chg.is_high()
    }

    pub fn get_vbus_pgood(&self) -> bool {
        self.pin_vbus_pgood.is_low() // Active low
    }

    /// Cartridge slot switch: true for pressed (Game Boy), false for not (GBA).
    pub fn get_cart_switch(&mut self) -> bool {
        if let Some(pin) = self.pin_cart_switch.as_ref() {
            pin.is_high()
        } else {
            self.fpga.get_cartridge_slot_button().unwrap()
        }
    }

    pub fn set_cart_power(&mut self, enabled: bool) {
        if let Some(pin) = self.pin_cart_power.as_mut() {
            log::info!("Cartridge power: {}", enabled);
            pin.set_level(Level::from(enabled)).unwrap();
        }
    }
}
