#![allow(unused)]

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
    #[error("dc pin error")]
    DcError,
    #[error("spi error")]
    SpiError,
}

#[derive(Debug, Copy, Clone)]
pub enum RenderSource {
    Fpga,
    Mcu,
}

pub struct ILI9488<PinReset: OutputPin, PinDc: OutputPin, Spi: SpiDevice, T: Timer> {
    pin_reset: PinReset,
    pin_dc: PinDc,
    spi: Spi,
    timer: T,
    /// When the panel last changed sleep state, as a reading from `timer`.
    last_sleep_change: u64,
    render_source: RenderSource,
}

impl<PinReset, PinDc, Spi, T> ILI9488<PinReset, PinDc, Spi, T>
where
    Spi: SpiDevice,
    PinReset: OutputPin,
    PinDc: OutputPin,
    T: Timer,
{
    pub fn new(pin_reset: PinReset, pin_dc: PinDc, spi: Spi, timer: T) -> Self {
        ILI9488 {
            last_sleep_change: timer.now_ms(),
            pin_reset,
            pin_dc,
            spi,
            timer,
            render_source: RenderSource::Mcu,
        }
    }

    pub fn init(&mut self) -> Result<(), Error> {
        self.pin_reset.set_low().map_err(|_| Error::ResetError)?;
        self.timer.sleep(Duration::from_micros(100));
        self.pin_reset.set_high().map_err(|_| Error::ResetError)?;
        self.last_sleep_change = self.timer.now_ms();

        // "Adjust Control 3": params have no specified meaning
        self.write_cmd(0xF7, &[0xA9, 0x51, 0x2C, 0x82])?;

        // MADCTL (Memory Access Control):
        // BGR order, rotate display orientation
        // (vendor provided is 0x48)
        // TODO might want to also update LCD shift register direction somewhere?
        // self.write_cmd(0x36, &[0xE8]); // Rotated
        self.write_cmd(0x36, &[0xC8])?; // original  (works with *native* bitstream, W=320,H=480)

        // Interface Pixel Format: DPI = 18 bit, DBI = 18 bit
        //  (16 bit doesn't work)
        self.write_cmd(0x3A, &[0x66])?;

        // Interface Mode Control: use separate SPI read/write wires
        // TODO: THIS SHOULD BE 0b1000_0000 -- use the same SDA wire,
        //    because ILI9488 does *NOT* tri-state SDO when CS is high,
        //    meaning it screws up the SPI bus. It should be *disconnected*
        //    (or in a board revision, a tri-state buffer added.)
        // also configure enable/dotclk/hsync/vsync polarity (for FPGA)
        self.write_cmd(0xB0, &[0x0E])?;

        // Display Inversion Control: 2-dot (from vendor)
        self.write_cmd(0xB4, &[0x02])?;

        // Frame rate: 60 Hz
        self.write_cmd(0xB1, &[0xA0, 0x11])?;

        // Power Control 1: Vreg1out=4.56  Vreg2out=-4.56 (from vendor)
        self.write_cmd(0xC0, &[0x0f, 0x0f])?;

        // Power Control 2: VGH=15.81 ,VGL=-10.41,DDVDH=5.35,DDVDL=-5.23  VCL=-2.7 (from vendor)
        self.write_cmd(0xC1, &[0x41])?;

        // Power Control 3: (from vendor)
        self.write_cmd(0xC2, &[0x22])?;

        // VCOM Control (from vendor)
        self.write_cmd(0xC5, &[0x00, 0x53, 0x80])?;

        // Entry Mode Set (from vendor)
        self.write_cmd(0xB7, &[0xC6])?;

        // Positive Gamma Control (from vendor)
        self.write_cmd(
            0xE0,
            &[
                0x00, 0x08, 0x0C, 0x02, 0x0E, 0x04, 0x30, 0x45, 0x47, 0x04, 0x0C, 0x0A, 0x2E, 0x34,
                0x0F,
            ],
        )?;

        // Negative Gamma Control (from vendor)
        self.write_cmd(
            0xE1,
            &[
                0x00, 0x11, 0x0D, 0x01, 0x0F, 0x05, 0x39, 0x36, 0x51, 0x06, 0x0F, 0x0D, 0x33, 0x37,
                0x0F,
            ],
        )?;

        // Display Inversion ON (?? from vendor)
        self.write_cmd(0x21, &[])?;

        Ok(())
    }

    pub fn enter_sleep(&mut self) -> Result<(), Error> {
        self.timer
            .sleep_until(self.last_sleep_change, SLEEP_CHANGE_DELAY);

        // Display off
        self.write_cmd(0x28, &[])?;

        // Disable use of DOTCLK (avoids temporary flickering after undocking)
        self.write_cmd(0xB6, &[0x02, 0x02])?;

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

        // Re-enable use of DOTCLK
        if matches!(self.render_source, RenderSource::Fpga) {
            self.write_cmd(0xB6, &[0xB2, 0x62])?;
        }

        // Display on
        self.write_cmd(0x29, &[])?;

        Ok(())
    }

    pub fn write_cmd(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.pin_dc.set_low().map_err(|_| Error::DcError)?;
        self.spi.write(&[cmd]).map_err(|_| Error::SpiError)?;
        if !params.is_empty() {
            self.pin_dc.set_high().map_err(|_| Error::DcError)?;
            self.spi.write(params).map_err(|_| Error::SpiError)?;
        }
        Ok(())
    }

    /// Set the LCD to be controlled by the FPGA.
    pub fn enable_fpga_control(&mut self) -> Result<(), Error> {
        // Display Function Control: enable RGB interface, bypassing memory
        self.write_cmd(0xB6, &[0xB2, 0x62])
            .map_err(|_| Error::SpiError)?;

        // Display Inversion Control: setup column inversion
        self.write_cmd(0xB4, &[0x0])?;

        self.render_source = RenderSource::Fpga;
        Ok(())
    }

    /// Set the LCD to be controlled by MCU.
    pub fn enable_mcu_control(&mut self) -> Result<(), Error> {
        // Display Function Control
        self.write_cmd(0xB6, &[0x02, 0x02, 0x3B])?;

        // Display Inversion Control: 2-dot (from vendor)
        self.write_cmd(0xB4, &[0x02])?;

        // MADCTL (Memory access control)
        // BGR, rotate
        self.write_cmd(0x36, &[0xE8])?;

        self.render_source = RenderSource::Mcu;
        Ok(())
    }

    /// Set LCD graphics memory read/write position
    pub fn set_gram_pos(&mut self, x0: u16, x1: u16, y0: u16, y1: u16) -> Result<(), Error> {
        // Column address set
        self.write_cmd(
            0x2A,
            &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8],
        )?;
        // Page address set
        self.write_cmd(
            0x2B,
            &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8],
        )?;
        Ok(())
    }

    /// Write to the LCD graphics memory
    pub fn write_gram(&mut self, data: &[u8]) -> Result<(), Error> {
        // Begin memory write
        self.write_cmd(0x2C, data)
    }

    pub fn get_render_source(&self) -> RenderSource {
        self.render_source
    }

    // TODO: read_cmd
}
