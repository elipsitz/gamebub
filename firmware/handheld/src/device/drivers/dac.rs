#![allow(dead_code)]

use core::time::Duration;

use super::timer::Timer;

use embedded_hal::digital::OutputPin;
use embedded_hal::i2c::I2c;
use thiserror::Error;

const ADDRESS: u8 = 0x18;

#[derive(Debug, Error)]
pub enum Error {
    #[error("gpio error")]
    PinError,
    #[error("i2c communication error")]
    I2cError,
}

#[derive(Copy, Clone)]
pub struct InterruptStatus {
    /// Short circuit detected at HPL / left class-D driver
    pub left_short_circuit: bool,
    /// Short circuit detected at HPR / right class-D driver
    pub right_short_circuit: bool,
    /// Headset button pressed
    pub headset_button_pressed: bool,
    /// Headset insertion or removal is detected
    pub headset_detected: bool,
    /// Left DAC signal power is above signal threshold of DRC
    pub left_dac_power: bool,
    /// Right DAC signal power is above signal threshold of DRC
    pub right_dac_power: bool,
}

pub struct TLV320DAC3101<PinReset: OutputPin, I2C: I2c, T: Timer> {
    pin_reset: PinReset,
    i2c: I2C,
    timer: T,
    page: u8,

    volume: u8,
    mute: bool,
}

impl<PinReset, I2C, T> TLV320DAC3101<PinReset, I2C, T>
where
    PinReset: OutputPin,
    I2C: I2c,
    T: Timer,
{
    pub fn new(pin_reset: PinReset, i2c: I2C, timer: T) -> Self {
        TLV320DAC3101 {
            pin_reset,
            i2c,
            timer,
            page: 0,

            volume: 0,
            mute: false,
        }
    }

    /// Reset the device without configuring it.
    pub fn reset(&mut self) -> Result<(), Error> {
        self.pin_reset.set_low().map_err(|_| Error::PinError)?;
        self.timer.sleep(Duration::from_micros(1));
        self.pin_reset.set_high().map_err(|_| Error::PinError)?;
        self.timer.sleep(Duration::from_millis(1));
        Ok(())
    }

    /// Hold the device in reset.
    ///
    /// Must call `reset()` later to release reset.
    pub fn reset_hold(&mut self) -> Result<(), Error> {
        self.pin_reset.set_low().map_err(|_| Error::PinError)
    }

    /// Sets up the DAC with both channels muted, and speakers and headphones
    /// both disabled.
    ///
    /// # DAC Configuration
    ///
    /// ## Filter Selection
    ///     PRB_P7 (interp. filter B) or PRB_P17 (interp. filter C) look like
    ///     basic, low power, stereo filters. They use IIR without any
    ///     biquads.
    ///     Section 6.3.10.1.4 talks about different interpolation filters.
    ///     Type B is for up to 96kHz, Type C is specifically for 192 kHz.
    ///     So we're going with *PRB_P7* for now.
    ///
    /// ## Clock dividers
    ///     CODEC_CLKIN = NDAC × MDAC × DOSR × DAC_fS
    ///     DAC_fS is 48KHz, and CODEC_CLKIN is MCLK.
    ///     
    ///     [Rev 1, 2]: MCLK is 256 * DAC_fS
    ///     [Rev 3]:    MCLK is 544 * DAC_fS
    ///     [Rev 4]:    MCLK is 608 * DAC_fS
    ///
    ///     For filter type B, DOSR must be a multiple of 4.
    ///     (DOSR is "oversampling ratio"?)
    ///     2.8 MHz < DOSR × DAC_fS < 6.2 MHz
    ///     Thus, DOSR can be from 60 to 128
    ///
    ///     NDAC and MDAC can be from 1..128.
    ///     NDAC should be as large as possible with "MDAC × DOSR / 32 ≥ RC",
    ///     where RC for PRB_P7 is 6.
    ///
    ///     [Rev 1, 2]: DOSR = 128, MDAC = 2, NDAC = 1
    ///     [Rev 3]:    DOSR =  68, MDAC = 4, NDAC = 2
    ///     [Rev 4]:    DOSR =  76, MDAC = 4, NDAC = 2
    ///
    /// ## Common-mode voltage
    ///     Based on the analog power supply. For Rev 1+, we have 3.3V.
    ///     The options are 1.35 V, 1.5 V, 1.65 V, or 1.8 V, and it must be
    ///     <= AVDD/2.
    ///     We'll go with 1.5V
    pub fn init(&mut self) -> Result<(), Error> {
        cfg_if::cfg_if! {
            if #[cfg(any(feature = "rev1", feature = "rev2"))] {
                let dosr = 128;
                let mdac = 2;
                let ndac = 1;
            } else if #[cfg(feature = "rev3")] {
                let dosr = 68;
                let mdac = 4;
                let ndac = 2;
            } else if #[cfg(feature = "rev4")] {
                let dosr = 76;
                let mdac = 4;
                let ndac = 2;
            }
        };

        // 1. Set up device.
        self.reset()?;

        // Do software reset?
        // self.write_reg(0,  0x01, 0x01);

        // 2. Program clock settings
        // PLL_clkin = MCLK, codec_clkin=MCLK
        self.write_reg(0, 0x04, 0x00)?;

        // PLL is unused
        // self.write_reg(0, 0x06, 0x08);
        // self.write_reg(0, 0x07, 0x00);
        // self.write_reg(0, 0x08, 0x00);
        // self.write_reg(0, 0x05, 0x91);

        // Program and power up NDAC
        self.write_reg(0, 0x0B, 0x80 | ndac)?;

        // Program and power up MDAC
        self.write_reg(0, 0x0C, 0x80 | mdac)?;

        // Program OSR
        //
        // DOSR = 128, DOSR(9:8) = 0, DOSR(7:0) = 128
        self.write_reg(0, 0x0D, ((dosr >> 8) & 0xFF) as u8)?;
        self.write_reg(0, 0x0E, (dosr & 0xFF) as u8)?;

        // Program codec interface (DSP, 16-bit, BCLK/WCLK as inputs);
        self.write_reg(0, 0x1B, 0x40)?;

        // Program processing block. Select PRB_P7
        self.write_reg(0, 0x3C, 0x07)?;
        // Enable adaptive filtering
        self.write_reg(0, 0x00, 0x08)?;
        self.write_reg(0, 0x01, 0x04)?;
        self.write_reg(0, 0x00, 0x00)?;

        // DAC volume control through register, not pin
        self.write_reg(0, 0x74, 0x00)?;

        // 3. Program analog blocks

        // Program common-mode voltage (set to 1.5 V);
        self.write_reg(1, 0x1F, 0x0C)?;

        // Program headphone depop settings (power on = 800ms, step = 4ms);
        self.write_reg(1, 0x21, 0x4E)?;

        // Route DAC output to output amplifier mixer
        // LDAC to HPL, RDAC to HPR
        self.write_reg(1, 0x23, 0x44)?;

        // Unmute and set gain of output driver
        // Unmute HPL, set gain = 0 db
        self.write_reg(1, 0x28, 0x06)?;
        // Unmute HPR, set gain = 0 dB
        self.write_reg(1, 0x29, 0x06)?;
        // Unmute left speaker, set gain = 6 dB
        self.write_reg(1, 0x2A, 0x04)?;
        // Unmute right speaker, set gain = 6 dB
        self.write_reg(1, 0x2B, 0x04)?;

        // DAC left digital volume control = 0 dB
        self.write_reg(0, 0x41, 0)?;
        // DAC right digital volume control = 0 dB
        self.write_reg(0, 0x42, 0)?;

        // TODO: Apply waiting time determined by the de-pop settings and the soft-stepping settings
        //    of the driver gain or poll page 1 / register 63
        // ...

        // 5. Power up DAC

        // Powerup DAC left and right channels (soft step enabled);
        self.write_reg(0, 0x3F, 0xD4)?;

        // Enable headphone detection
        self.configure_headphone_detection(true)?;

        Ok(())
    }

    /// Get the volume level
    pub fn get_volume(&self) -> u8 {
        self.volume
    }

    /// Set analog volume for left and right.
    pub fn set_volume(&mut self, volume: u8) -> Result<(), Error> {
        self.volume = volume;
        self.set_mute(volume == 0)?;

        // Volume level 0 is mapped to value 117 (–78.3 dB analog gain)
        // Volume level 255 is mapped to value 0 (0 dB analog gain).
        let value = 117 - ((((volume as i32) * 117) / 255) as u8);
        // Headphone left
        self.write_reg(1, 0x24, value)?;
        // Headphone right
        self.write_reg(1, 0x25, value)?;
        // Speaker left
        self.write_reg(1, 0x26, value)?;
        // Speaker right
        self.write_reg(1, 0x27, value)?;
        Ok(())
    }

    pub fn set_mute(&mut self, mute: bool) -> Result<(), Error> {
        // Left and right are individually controllable, but this sets them together.
        self.mute = mute;
        self.write_reg(0, 0x40, if mute { 0xC } else { 0x0 })
    }

    pub fn get_mute(&mut self) -> bool {
        self.mute
    }

    pub fn set_headphones_enabled(&mut self, enabled: bool) -> Result<(), Error> {
        self.write_reg(1, 0x1F, if enabled { 0xC2 } else { 0x02 })
    }

    pub fn set_speakers_enabled(&mut self, enabled: bool) -> Result<(), Error> {
        self.write_reg(1, 0x20, if enabled { 0xC6 } else { 0x06 })
    }

    /// Enable or disable headphone detection.
    pub fn configure_headphone_detection(&mut self, enabled: bool) -> Result<(), Error> {
        let debounce_headset = 0b011u8; // 128ms
        let debounce_button = 0u8; // 0ms
        let value = ((enabled as u8) << 7) | (debounce_headset << 2) | debounce_button;
        self.write_reg(0, 0x43, value)?;

        let micbias = if enabled { 0xB } else { 0x0 };
        self.write_reg(1, 0x2E, micbias)?;

        Ok(())
    }

    /// Get whether headphones are plugged in.
    pub fn get_headphones_detected(&mut self) -> Result<bool, Error> {
        self.read_reg(0, 0x43).map(|x| (x & 0x20) != 0)
    }

    pub fn configure_interrupts(&mut self) -> Result<(), Error> {
        // Configure INT1 to use headphone detection interrupt.
        self.write_reg(0, 0x30, 0x80)?;
        // Configure GPIO1 as INT1 output
        self.write_reg(0, 0x33, 0x14)
    }

    pub fn get_interrupt_status(&mut self) -> Result<InterruptStatus, Error> {
        let value = self.read_reg(0, 0x2C)?;
        Ok(InterruptStatus {
            left_short_circuit: (value & (1 << 7)) != 0,
            right_short_circuit: (value & (1 << 6)) != 0,
            headset_button_pressed: (value & (1 << 5)) != 0,
            headset_detected: (value & (1 << 4)) != 0,
            left_dac_power: (value & (1 << 3)) != 0,
            right_dac_power: (value & (1 << 2)) != 0,
        })
    }

    fn set_page(&mut self, page: u8) -> Result<(), Error> {
        if self.page != page {
            self.page = page;
            self.i2c
                .write(ADDRESS, &[0, page])
                .map_err(|_| Error::I2cError)
        } else {
            Ok(())
        }
    }

    fn write_reg(&mut self, page: u8, reg: u8, data: u8) -> Result<(), Error> {
        self.set_page(page)?;
        self.i2c
            .write(ADDRESS, &[reg, data])
            .map_err(|_| Error::I2cError)
    }

    #[allow(unused)]
    fn read_reg(&mut self, page: u8, reg: u8) -> Result<u8, Error> {
        self.set_page(page)?;
        let mut data = [0u8; 1];
        self.i2c
            .write_read(ADDRESS, &[reg], &mut data)
            .map_err(|_| Error::I2cError)?;
        Ok(data[0])
    }
}
