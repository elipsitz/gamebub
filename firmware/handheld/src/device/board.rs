use arrayvec::ArrayVec;
use esp_idf_svc::hal::gpio::Pin;
use esp_idf_svc::hal::{
    gpio::{AnyIOPin, AnyInputPin, AnyOutputPin, IOPin, InputPin, OutputPin},
    peripherals::Peripherals,
};

cfg_if::cfg_if! {
    if #[cfg(feature = "rev1")] {
        pub const PIN_POWER_SWITCH: i32 = 1;
    } else if #[cfg(feature = "rev2")] {
        pub const PIN_POWER_SWITCH: i32 = 15;
    } else if #[cfg(any(feature = "rev3", feature = "rev4"))] {
        pub const PIN_POWER_SWITCH: i32 = 8;
    }
}

pub struct BoardPins {
    // Buttons
    pub home: AnyIOPin,
    pub vol_up: AnyIOPin,
    pub vol_down: AnyIOPin,
    pub power_switch: AnyIOPin,

    // LED
    pub led: AnyOutputPin,

    // Power and charging
    pub vbus_pgood: AnyIOPin,
    pub batt_chg: AnyInputPin,
    #[allow(unused)]
    pub batt_charge_enable: Option<AnyOutputPin>,

    // LCD
    // TODO: split into a per-revision enum
    pub lcd_backlight: AnyOutputPin,
    pub lcd_reset: Option<AnyOutputPin>,
    pub lcd_cs: Option<AnyOutputPin>,
    #[allow(unused)]
    pub lcd_dc: Option<AnyOutputPin>,
    #[allow(unused)]
    pub lcd_enable: Option<AnyOutputPin>,
    pub lcd_spi_clk: Option<AnyOutputPin>,
    pub lcd_spi_d0: Option<AnyOutputPin>,

    // FPGA
    pub fpga_power: AnyOutputPin,
    pub fpga_init_b: AnyIOPin,
    pub fpga_done: AnyInputPin,
    pub fpga_program_b: AnyOutputPin,
    pub fpga_spi_cs: AnyOutputPin,
    pub fpga_spi_clk: AnyOutputPin,
    pub fpga_spi_d0: AnyIOPin,
    pub fpga_spi_d1: AnyIOPin,
    pub fpga_spi_d2: AnyIOPin,
    pub fpga_spi_d3: AnyIOPin,

    // I2C
    pub irq: AnyInputPin,
    pub i2c_scl: AnyIOPin,
    pub i2c_sda: AnyIOPin,

    // microSD
    pub sdio_clk: AnyOutputPin,
    pub sdio_cmd: AnyIOPin,
    pub sdio_d0: AnyIOPin,
    pub sdio_d1: AnyIOPin,
    pub sdio_d2: AnyIOPin,
    pub sdio_d3: AnyIOPin,
    pub sd_detect: Option<AnyInputPin>,

    // DAC
    pub dac_reset: AnyOutputPin,

    // Cartridge slot
    pub cart_switch: Option<AnyInputPin>,
    pub cart_power: Option<AnyOutputPin>,
}

#[derive(Clone, Copy, Debug)]
pub struct SdPins {
    pub clk: i32,
    pub cmd: i32,
    pub d0: i32,
    pub d1: i32,
    pub d2: i32,
    pub d3: i32,
    pub cd: Option<i32>,
}

impl BoardPins {
    pub fn sd_pins(&self) -> SdPins {
        SdPins {
            clk: self.sdio_clk.pin(),
            cmd: self.sdio_cmd.pin(),
            d0: self.sdio_d0.pin(),
            d1: self.sdio_d1.pin(),
            d2: self.sdio_d2.pin(),
            d3: self.sdio_d3.pin(),
            cd: self.sd_detect.as_ref().map(|p| p.pin()),
        }
    }
    /// Get the pin numbers of all lines connected connected to power domains unpowered
    /// during deep sleep.
    pub fn unpowered_pins(&self) -> ArrayVec<i32, 18> {
        let mut pins = ArrayVec::new();
        pins.push(self.fpga_init_b.pin());
        pins.push(self.fpga_done.pin());
        pins.push(self.fpga_program_b.pin());
        pins.push(self.fpga_spi_cs.pin());
        pins.push(self.fpga_spi_clk.pin());
        pins.push(self.fpga_spi_d0.pin());
        pins.push(self.fpga_spi_d1.pin());
        pins.push(self.fpga_spi_d2.pin());
        pins.push(self.fpga_spi_d3.pin());
        pins.push(self.lcd_backlight.pin());
        if let Some(p) = &self.lcd_reset {
            pins.push(p.pin());
        }
        if let Some(p) = &self.lcd_cs {
            pins.push(p.pin());
        }
        if let Some(p) = &self.lcd_dc {
            pins.push(p.pin());
        }
        if let Some(p) = &self.lcd_enable {
            pins.push(p.pin());
        }
        if let Some(p) = &self.lcd_spi_clk {
            pins.push(p.pin());
        }
        if let Some(p) = &self.lcd_spi_d0 {
            pins.push(p.pin());
        }
        if let Some(p) = &self.cart_switch {
            pins.push(p.pin());
        }
        if let Some(p) = &self.cart_power {
            pins.push(p.pin());
        }
        pins
    }
}

pub struct Board {
    pub pins: BoardPins,
    pub sleep_config: crate::device::deep_sleep::SleepConfig,
    pub i2c: esp_idf_svc::hal::i2c::I2C0,
    pub ledc: esp_idf_svc::hal::ledc::LEDC,
    pub spi2: esp_idf_svc::hal::spi::SPI2,
    #[cfg(any(feature = "rev2", feature = "rev4"))]
    pub spi3: esp_idf_svc::hal::spi::SPI3,
}

impl Board {
    pub fn new(peripherals: Peripherals) -> Self {
        let p = peripherals.pins;

        cfg_if::cfg_if! {
            if #[cfg(feature = "rev1")] {
                let pins = BoardPins {
                    led: p.gpio3.downgrade_output(),
                    irq: p.gpio2.downgrade_input(),
                    home: p.gpio0.downgrade(),
                    vol_up: p.gpio4.downgrade(),
                    vol_down: p.gpio5.downgrade(),
                    power_switch: p.gpio1.downgrade(),
                    vbus_pgood: p.gpio41.downgrade(),
                    batt_chg: p.gpio42.downgrade_input(),
                    batt_charge_enable: None,
                    lcd_backlight: p.gpio6.downgrade_output(),
                    lcd_reset: Some(p.gpio7.downgrade_output()),
                    lcd_cs: Some(p.gpio15.downgrade_output()),
                    lcd_dc: Some(p.gpio16.downgrade_output()),
                    lcd_enable: None,
                    lcd_spi_clk: None,
                    lcd_spi_d0: None,
                    fpga_power: p.gpio46.downgrade_output(),
                    fpga_init_b: p.gpio8.downgrade(),
                    fpga_done: p.gpio17.downgrade_input(),
                    fpga_program_b: p.gpio18.downgrade_output(),
                    fpga_spi_cs: p.gpio10.downgrade_output(),
                    fpga_spi_clk: p.gpio12.downgrade_output(),
                    fpga_spi_d0: p.gpio11.downgrade(),
                    fpga_spi_d1: p.gpio13.downgrade(),
                    fpga_spi_d2: p.gpio14.downgrade(),
                    fpga_spi_d3: p.gpio9.downgrade(),
                    i2c_scl: p.gpio39.downgrade(),
                    i2c_sda: p.gpio38.downgrade(),
                    sdio_clk: p.gpio45.downgrade_output(),
                    sdio_cmd: p.gpio48.downgrade(),
                    sdio_d0: p.gpio35.downgrade(),
                    sdio_d1: p.gpio36.downgrade(),
                    sdio_d2: p.gpio21.downgrade(),
                    sdio_d3: p.gpio47.downgrade(),
                    sd_detect: Some(p.gpio37.downgrade_input()),
                    dac_reset: p.gpio40.downgrade_output(),
                    cart_switch: None,
                    cart_power: None,
                };
            } else if #[cfg(feature = "rev2")] {
                let pins = BoardPins {
                    led: p.gpio36.downgrade_output(),
                    irq: p.gpio16.downgrade_input(),
                    home: p.gpio0.downgrade(),
                    vol_up: p.gpio41.downgrade(),
                    vol_down: p.gpio40.downgrade(),
                    power_switch: p.gpio15.downgrade(),
                    vbus_pgood: p.gpio17.downgrade(),
                    batt_chg: p.gpio39.downgrade_input(),
                    batt_charge_enable: None,
                    lcd_backlight: p.gpio42.downgrade_output(),
                    lcd_reset: Some(p.gpio1.downgrade_output()),
                    lcd_cs: Some(p.gpio2.downgrade_output()),
                    lcd_dc: Some(p.gpio3.downgrade_output()),
                    lcd_enable: None,
                    lcd_spi_clk: Some(p.gpio4.downgrade_output()),
                    lcd_spi_d0: Some(p.gpio5.downgrade_output()),
                    fpga_power: p.gpio46.downgrade_output(),
                    fpga_init_b: p.gpio8.downgrade(),
                    fpga_done: p.gpio6.downgrade_input(),
                    fpga_program_b: p.gpio7.downgrade_output(),
                    fpga_spi_cs: p.gpio10.downgrade_output(),
                    fpga_spi_clk: p.gpio12.downgrade_output(),
                    fpga_spi_d0: p.gpio11.downgrade(),
                    fpga_spi_d1: p.gpio13.downgrade(),
                    fpga_spi_d2: p.gpio14.downgrade(),
                    fpga_spi_d3: p.gpio9.downgrade(),
                    i2c_scl: p.gpio37.downgrade(),
                    i2c_sda: p.gpio38.downgrade(),
                    sdio_clk: p.gpio33.downgrade_output(),
                    sdio_cmd: p.gpio47.downgrade(),
                    sdio_d0: p.gpio34.downgrade(),
                    sdio_d1: p.gpio48.downgrade(),
                    sdio_d2: p.gpio21.downgrade(),
                    sdio_d3: p.gpio26.downgrade(),
                    sd_detect: Some(p.gpio35.downgrade_input()),
                    dac_reset: p.gpio45.downgrade_output(),
                    cart_switch: None,
                    cart_power: None,
                };
            } else if #[cfg(feature = "rev3")] {
                let pins = BoardPins {
                    led: p.gpio42.downgrade_output(),
                    irq: p.gpio6.downgrade_input(),
                    home: p.gpio0.downgrade(),
                    vol_up: p.gpio2.downgrade(),
                    vol_down: p.gpio1.downgrade(),
                    power_switch: p.gpio8.downgrade(),
                    vbus_pgood: p.gpio9.downgrade(),
                    batt_chg: p.gpio3.downgrade_input(),
                    batt_charge_enable: Some(p.gpio10.downgrade_output()),
                    lcd_backlight: p.gpio45.downgrade_output(),
                    lcd_reset: None,
                    lcd_cs: None,
                    lcd_dc: None,
                    lcd_enable: Some(p.gpio46.downgrade_output()),
                    lcd_spi_clk: None,
                    lcd_spi_d0: None,
                    fpga_power: p.gpio13.downgrade_output(),
                    fpga_init_b: p.gpio18.downgrade(),
                    fpga_done: p.gpio16.downgrade_input(),
                    fpga_program_b: p.gpio17.downgrade_output(),
                    fpga_spi_cs: p.gpio21.downgrade_output(),
                    fpga_spi_clk: p.gpio48.downgrade_output(),
                    fpga_spi_d0: p.gpio34.downgrade(),
                    fpga_spi_d1: p.gpio33.downgrade(),
                    fpga_spi_d2: p.gpio47.downgrade(),
                    fpga_spi_d3: p.gpio26.downgrade(),
                    i2c_scl: p.gpio4.downgrade(),
                    i2c_sda: p.gpio5.downgrade(),
                    sdio_clk: p.gpio38.downgrade_output(),
                    sdio_cmd: p.gpio37.downgrade(),
                    sdio_d0: p.gpio39.downgrade(),
                    sdio_d1: p.gpio40.downgrade(),
                    sdio_d2: p.gpio35.downgrade(),
                    sdio_d3: p.gpio36.downgrade(),
                    sd_detect: Some(p.gpio41.downgrade_input()),
                    dac_reset: p.gpio7.downgrade_output(),
                    cart_switch: Some(p.gpio14.downgrade_input()),
                    cart_power: Some(p.gpio15.downgrade_output()),
                };
            } else if #[cfg(feature = "rev4")] {
                let pins = BoardPins {
                    led: p.gpio42.downgrade_output(),
                    irq: p.gpio6.downgrade_input(),
                    home: p.gpio0.downgrade(),
                    vol_up: p.gpio2.downgrade(),
                    vol_down: p.gpio1.downgrade(),
                    power_switch: p.gpio8.downgrade(),
                    vbus_pgood: p.gpio9.downgrade(),
                    batt_chg: p.gpio3.downgrade_input(),
                    batt_charge_enable: None,
                    lcd_backlight: p.gpio45.downgrade_output(),
                    lcd_reset: Some(p.gpio13.downgrade_output()),
                    lcd_cs: Some(p.gpio12.downgrade_output()),
                    lcd_dc: None,
                    lcd_enable: None,
                    lcd_spi_clk: Some(p.gpio11.downgrade_output()),
                    lcd_spi_d0: Some(p.gpio10.downgrade_output()),
                    fpga_power: p.gpio46.downgrade_output(),
                    fpga_init_b: p.gpio18.downgrade(),
                    fpga_done: p.gpio16.downgrade_input(),
                    fpga_program_b: p.gpio17.downgrade_output(),
                    fpga_spi_cs: p.gpio21.downgrade_output(),
                    fpga_spi_clk: p.gpio48.downgrade_output(),
                    fpga_spi_d0: p.gpio34.downgrade(),
                    fpga_spi_d1: p.gpio33.downgrade(),
                    fpga_spi_d2: p.gpio47.downgrade(),
                    fpga_spi_d3: p.gpio26.downgrade(),
                    i2c_scl: p.gpio4.downgrade(),
                    i2c_sda: p.gpio5.downgrade(),
                    sdio_clk: p.gpio38.downgrade_output(),
                    sdio_cmd: p.gpio37.downgrade(),
                    sdio_d0: p.gpio39.downgrade(),
                    sdio_d1: p.gpio40.downgrade(),
                    sdio_d2: p.gpio35.downgrade(),
                    sdio_d3: p.gpio36.downgrade(),
                    sd_detect: Some(p.gpio41.downgrade_input()),
                    dac_reset: p.gpio7.downgrade_output(),
                    cart_switch: Some(p.gpio14.downgrade_input()),
                    cart_power: Some(p.gpio15.downgrade_output()),
                };
            }
        }

        let sleep_config = crate::device::deep_sleep::SleepConfig {
            led_pin: pins.led.pin(),
            fpga_power_pin: pins.fpga_power.pin(),
            dac_reset_pin: pins.dac_reset.pin(),
            unpowered_pins: pins.unpowered_pins(),
        };

        Self {
            pins,
            sleep_config,
            i2c: peripherals.i2c0,
            ledc: peripherals.ledc,
            spi2: peripherals.spi2,
            #[cfg(any(feature = "rev2", feature = "rev4"))]
            spi3: peripherals.spi3,
        }
    }
}
