use core::time::Duration;

use super::timer::Timer;
use thiserror::Error;

use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiDevice;

/// Duration between successive calls to sleep in / sleep out.
const SLEEP_CHANGE_DELAY: Duration = Duration::from_millis(120);

#[derive(Debug, Error)]
pub enum Error {
    #[error("reset error")]
    ResetError,
    #[error("spi error")]
    SpiError,
}

pub struct ILI9806E<PinReset: OutputPin, Spi: SpiDevice, T: Timer> {
    pin_reset: PinReset,
    spi: Spi,
    timer: T,
    /// When the panel last changed sleep state, as a reading from `timer`.
    last_sleep_change: u64,
}

impl<PinReset, Spi, T> ILI9806E<PinReset, Spi, T>
where
    Spi: SpiDevice,
    PinReset: OutputPin,
    T: Timer,
{
    pub fn new(pin_reset: PinReset, spi: Spi, timer: T) -> Self {
        Self {
            last_sleep_change: timer.now_ms(),
            pin_reset,
            spi,
            timer,
        }
    }

    pub fn init(&mut self) -> Result<(), Error> {
        // Reset the display.
        self.pin_reset.set_high().map_err(|_| Error::ResetError)?;
        self.timer.sleep(Duration::from_millis(1));
        self.pin_reset.set_low().map_err(|_| Error::ResetError)?;
        self.timer.sleep(Duration::from_millis(5));
        self.pin_reset.set_high().map_err(|_| Error::ResetError)?;
        self.last_sleep_change = self.timer.now_ms();
        self.timer.sleep(Duration::from_millis(5));

        // Change to page 1
        self.write_cmd(0xFF, &[0xFF, 0x98, 0x06, 0x04, 0x01])?;
        // Output SDA
        self.write_cmd(0x08, &[0x10])?;
        // Display function control 1: DE mode
        self.write_cmd(0x20, &[0x00])?;
        // Display function control 2:
        // Set VSYNC/HSYNC active high (0b11xx)
        // Set data fetched at clock falling edge (0bxx1x)
        // Set data enable polarity active high (0bxxx1)
        self.write_cmd(0x21, &[0b1111])?;
        // Resolution control
        self.write_cmd(0x30, &[0x02])?;
        // Inversion setting: 2 dot
        self.write_cmd(0x31, &[0x00])?;

        // Vendor parameters
        // Source timing adjust
        self.write_cmd(0x60, &[0x07])?;
        self.write_cmd(0x61, &[0x06])?;
        self.write_cmd(0x62, &[0x06])?;
        self.write_cmd(0x63, &[0x04])?;
        // Power control
        self.write_cmd(0x40, &[0x14])?;
        self.write_cmd(0x41, &[0x33])?;
        self.write_cmd(0x42, &[0x01])?;
        self.write_cmd(0x43, &[0x09])?;
        self.write_cmd(0x44, &[0x0A])?;
        self.write_cmd(0x45, &[0x0A])?;
        self.write_cmd(0x50, &[0x78])?;
        self.write_cmd(0x51, &[0x78])?;
        self.write_cmd(0x52, &[0x00])?;
        self.write_cmd(0x53, &[0x3B])?;
        // LVD detect
        self.write_cmd(0x57, &[0x50])?;
        // Positive gamma control
        self.write_cmd(0xA0, &[0x00])?;
        self.write_cmd(0xA1, &[0x13])?;
        self.write_cmd(0xA2, &[0x19])?;
        self.write_cmd(0xA3, &[0x0C])?;
        self.write_cmd(0xA4, &[0x06])?;
        self.write_cmd(0xA5, &[0x0A])?;
        self.write_cmd(0xA6, &[0x06])?;
        self.write_cmd(0xA7, &[0x04])?;
        self.write_cmd(0xA8, &[0x09])?;
        self.write_cmd(0xA9, &[0x08])?;
        self.write_cmd(0xAA, &[0x12])?;
        self.write_cmd(0xAB, &[0x06])?;
        self.write_cmd(0xAC, &[0x0E])?;
        self.write_cmd(0xAD, &[0x0E])?;
        self.write_cmd(0xAE, &[0x09])?;
        self.write_cmd(0xAF, &[0x00])?;
        // Negative gamma correction
        self.write_cmd(0xC0, &[0x00])?;
        self.write_cmd(0xC1, &[0x0D])?;
        self.write_cmd(0xC2, &[0x18])?;
        self.write_cmd(0xC3, &[0x0D])?;
        self.write_cmd(0xC4, &[0x06])?;
        self.write_cmd(0xC5, &[0x09])?;
        self.write_cmd(0xC6, &[0x07])?;
        self.write_cmd(0xC7, &[0x05])?;
        self.write_cmd(0xC8, &[0x08])?;
        self.write_cmd(0xC9, &[0x0E])?;
        self.write_cmd(0xCA, &[0x12])?;
        self.write_cmd(0xCB, &[0x09])?;
        self.write_cmd(0xCC, &[0x0E])?;
        self.write_cmd(0xCD, &[0x0E])?;
        self.write_cmd(0xCE, &[0x08])?;
        self.write_cmd(0xCF, &[0x00])?;

        // Change to page 7
        self.write_cmd(0xFF, &[0xFF, 0x98, 0x06, 0x04, 0x07])?;
        self.write_cmd(0x17, &[0x32])?;
        self.write_cmd(0x18, &[0x1D])?;
        self.write_cmd(0x26, &[0xB2])?;
        self.write_cmd(0x02, &[0x77])?;
        self.write_cmd(0xE1, &[0x79])?;
        self.write_cmd(0xE1, &[0x79])?;
        self.write_cmd(0xB3, &[0x10])?;

        // Change to page 6
        self.write_cmd(0xFF, &[0xFF, 0x98, 0x06, 0x04, 0x06])?;
        self.write_cmd(0x00, &[0x20])?;
        self.write_cmd(0x01, &[0x04])?;
        self.write_cmd(0x02, &[0x00])?;
        self.write_cmd(0x03, &[0x00])?;
        self.write_cmd(0x04, &[0x01])?;
        self.write_cmd(0x05, &[0x01])?;
        self.write_cmd(0x06, &[0x88])?;
        self.write_cmd(0x07, &[0x04])?;
        self.write_cmd(0x08, &[0x01])?;
        self.write_cmd(0x09, &[0x90])?;
        self.write_cmd(0x0A, &[0x03])?;
        self.write_cmd(0x0B, &[0x01])?;
        self.write_cmd(0x0C, &[0x01])?;
        self.write_cmd(0x0D, &[0x01])?;
        self.write_cmd(0x0E, &[0x00])?;
        self.write_cmd(0x0F, &[0x00])?;
        self.write_cmd(0x10, &[0x55])?;
        self.write_cmd(0x11, &[0x53])?;
        self.write_cmd(0x12, &[0x01])?;
        self.write_cmd(0x13, &[0x0D])?;
        self.write_cmd(0x14, &[0x0D])?;
        self.write_cmd(0x15, &[0x43])?;
        self.write_cmd(0x16, &[0x0B])?;
        self.write_cmd(0x17, &[0x00])?;
        self.write_cmd(0x18, &[0x00])?;
        self.write_cmd(0x19, &[0x00])?;
        self.write_cmd(0x1A, &[0x00])?;
        self.write_cmd(0x1B, &[0x00])?;
        self.write_cmd(0x1C, &[0x00])?;
        self.write_cmd(0x1D, &[0x00])?;
        self.write_cmd(0x20, &[0x01])?;
        self.write_cmd(0x21, &[0x23])?;
        self.write_cmd(0x22, &[0x45])?;
        self.write_cmd(0x23, &[0x67])?;
        self.write_cmd(0x24, &[0x01])?;
        self.write_cmd(0x25, &[0x23])?;
        self.write_cmd(0x26, &[0x45])?;
        self.write_cmd(0x27, &[0x67])?;
        self.write_cmd(0x30, &[0x02])?;
        self.write_cmd(0x31, &[0x22])?;
        self.write_cmd(0x32, &[0x11])?;
        self.write_cmd(0x33, &[0xAA])?;
        self.write_cmd(0x34, &[0xBB])?;
        self.write_cmd(0x35, &[0x66])?;
        self.write_cmd(0x36, &[0x00])?;
        self.write_cmd(0x37, &[0x22])?;
        self.write_cmd(0x38, &[0x22])?;
        self.write_cmd(0x39, &[0x22])?;
        self.write_cmd(0x3A, &[0x22])?;
        self.write_cmd(0x3B, &[0x22])?;
        self.write_cmd(0x3C, &[0x22])?;
        self.write_cmd(0x3D, &[0x22])?;
        self.write_cmd(0x3E, &[0x22])?;
        self.write_cmd(0x3F, &[0x22])?;
        self.write_cmd(0x40, &[0x22])?;
        self.write_cmd(0x52, &[0x10])?;
        self.write_cmd(0x53, &[0x12])?;
        self.write_cmd(0x54, &[0x13])?;

        // Change back to page 0 for normal commands
        self.write_cmd(0xFF, &[0xFF, 0x98, 0x06, 0x04, 0x00])?;
        // Display access control: BGR=0 GS=1
        self.write_cmd(0x36, &[0x01])?;
        // Interface pixel format: 24-bit
        self.write_cmd(0x3A, &[0x70])?;

        Ok(())
    }

    pub fn enter_sleep(&mut self) -> Result<(), Error> {
        self.timer
            .sleep_until(self.last_sleep_change, SLEEP_CHANGE_DELAY);

        // Display off
        self.write_cmd(0x28, &[])?;

        // Sleep in
        self.write_cmd(0x10, &[])?;
        self.last_sleep_change = self.timer.now_ms();

        Ok(())
    }

    pub fn exit_sleep(&mut self) -> Result<(), Error> {
        self.timer
            .sleep_until(self.last_sleep_change, SLEEP_CHANGE_DELAY);

        // Sleep out
        self.write_cmd(0x11, &[])?;
        self.last_sleep_change = self.timer.now_ms();

        // Must wait 5ms before sending commands after sleep out.
        self.timer.sleep(Duration::from_millis(5));

        // Display on
        self.write_cmd(0x29, &[])?;

        Ok(())
    }

    pub fn write_cmd(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        // 9-bit spi: D/C bit followed by 8 bits MSB-first
        // Pack everything into bytes.
        let mut data = [0u8; 8];
        if 9 + (9 * params.len()) > data.len() * 8 {
            return Err(Error::SpiError);
        }
        data[0] = cmd >> 1;
        data[1] = (cmd & 1) << 7;
        let mut index: usize = 9;

        let mut push = |bit: u8| {
            data[index / 8] |= (bit & 1) << (7 - (index % 8));
            index += 1;
        };

        for &p in params {
            push(1);
            for i in 0..8 {
                push((p >> (7 - i)) & 1);
            }
        }

        let buf = &data[0..(index + 7) / 8];
        self.spi.write(buf).map_err(|_| Error::SpiError)
    }

    /// Set the LCD to be controlled by the FPGA.
    pub fn enable_fpga_control(&mut self) -> Result<(), Error> {
        // No-op
        Ok(())
    }
}
