#![allow(dead_code)]

use core::time::Duration;

use embedded_io::Read;

use crate::device::drivers::timer::Timer;
use embedded_hal::{
    digital::{InputPin, OutputPin},
    spi::SpiDevice,
};
use esp_idf_svc::hal::{
    spi::{config::LineWidth, Operation, SpiDriver, SpiSharedDeviceDriver, SpiSoftCsDeviceDriver},
    units::Hertz,
};
use thiserror::Error;

use crate::device::DisplayMode;

pub const REG_CONTROL: u32 = 0x0100_0000;
pub const REG_FORCE_BUTTON: u32 = 0x0100_0004;
pub const REG_DISPLAY: u32 = 0x0100_0008;
pub const REG_IRQ_ENABLE: u32 = 0x0100_000C;
pub const REG_IRQ_STATUS: u32 = 0x0100_0010;
pub const REG_STATUS: u32 = 0x0100_0014;
pub const REG_COLOR_CORRECT_ENABLE: u32 = 0x0100_0018;
pub const REG_BUTTON_STATE: u32 = 0x0100_001C;
pub const REG_OVERLAY_XCTRL: u32 = 0x0100_0100;
pub const REG_OVERLAY_YCTRL: u32 = 0x0100_0104;
/// Framebuffer dimensions (read only)
pub const REG_FB_DIM: u32 = 0x0100_0200;
/// Color correction base
pub const REG_COLOR_CORRECT_PARAMS: u32 = 0x0200_0000;

/// The FPGA (due to the spi implementation) can read at a speed that's some
/// fraction of the SPI domain clock speed. At 200 MHz SPI receiver clock,
/// 16 MHz is a safe speed.
pub const MAX_SPI_READ_CLOCK: Hertz = Hertz(16_000_000);

pub type SpiDataDriver<'a> =
    SpiSoftCsDeviceDriver<'a, SpiSharedDeviceDriver<'a, &'a SpiDriver<'a>>, &'a SpiDriver<'a>>;

mod xilinx;

#[derive(Debug, Error)]
pub enum Error {
    #[error("gpio error")]
    PinError,
    #[error("error programming fpga")]
    ProgramError,
    #[error("error reading bitstream")]
    BitstreamError,
    #[error("incompatible bitstream")]
    IncompatibleBitstream,
    #[error("spi error")]
    SpiError,
}

#[derive(Copy, Clone)]
#[repr(u32)]
pub enum Irq {
    ModuleVblank = 0,
    Button = 1,
    SpiRequestOverflow = 2,
    SpiResponseUnderflow = 3,
}

impl Irq {
    pub const fn as_flag(self) -> u32 {
        1 << (self as u32)
    }
}

pub struct Fpga<
    'a,
    PinDone: InputPin,
    PinProgramB: OutputPin,
    PinInitB: InputPin,
    ProgramSpi: SpiDevice,
    T: Timer,
> {
    pin_done: PinDone,
    pub pin_program_b: PinProgramB,
    pin_init_b: PinInitB,
    /// List of SPI drivers and their clock speed, from largest to smallest.
    data_spi: Vec<(SpiDataDriver<'a>, Hertz)>,
    program_spi: ProgramSpi,

    /// Top-level "system" clock speed, which determines how fast reads
    /// and writes can occur.
    system_clock: Hertz,

    /// Bitfield of enabled interrupts
    interrupts: u32,

    timer: T,
}

impl<'a, PinDone, PinProgramB, PinInitB, ProgramSpi, T>
    Fpga<'a, PinDone, PinProgramB, PinInitB, ProgramSpi, T>
where
    PinDone: InputPin,
    PinProgramB: OutputPin,
    PinInitB: InputPin,
    ProgramSpi: SpiDevice,
    T: Timer,
{
    pub fn new(
        pin_done: PinDone,
        pin_program_b: PinProgramB,
        pin_init_b: PinInitB,
        data_spi: Vec<(SpiDataDriver<'a>, Hertz)>,
        program_spi: ProgramSpi,
        timer: T,
    ) -> Self {
        Fpga {
            timer,
            pin_done,
            pin_program_b,
            pin_init_b,
            data_spi,
            program_spi,
            system_clock: Hertz(8 * 1024 * 1024),
            interrupts: 0,
        }
    }

    /// Program the FPGA with a new bitstream.
    pub fn program<R: Read>(
        &mut self,
        bitstream: &mut R,
        scratch_buf: &mut [u8],
    ) -> Result<(), Error> {
        let header =
            xilinx::parse_bitstream_header(bitstream).map_err(|_| Error::BitstreamError)?;

        // Check that the bitstream was built for this hardware.
        let hardware_version = crate::hwinfo::get_hardware_version();
        let expected_id = 0xB010_0000 | (hardware_version.major as u32);
        if header.user_id.is_none() || header.user_id == Some(0xFFFF_FFFF) {
            log::info!("Bitstream has no UserID, assuming it is compatible");
        } else if hardware_version.major == 0 || hardware_version.major == 255 {
            log::info!("Hardware version major=0, skipping bitstream compatibility");
        } else if header.user_id != Some(expected_id) {
            log::error!("Incompatible bitstream, ID={:08X}", header.user_id.unwrap());
            return Err(Error::IncompatibleBitstream);
        }

        // After power-on-reset, INIT_B will be low for 10ms to 35ms (T_POR),
        // configuration can only start after this.
        // Poll INIT_B until it goes high.
        let start_ms = self.timer.now_ms();
        while self.pin_init_b.is_low().map_err(|_| Error::PinError)? {
            if self.timer.now_ms().saturating_sub(start_ms) > 35 {
                return Err(Error::ProgramError);
            }
            self.timer.sleep(Duration::from_millis(5));
        }

        // Pull PROGRAM_B low, hold it for at least 250ns.
        self.pin_program_b.set_low().map_err(|_| Error::PinError)?;
        self.timer.sleep(Duration::from_millis(1));
        if self.pin_init_b.is_high().map_err(|_| Error::PinError)? {
            return Err(Error::ProgramError);
        }
        self.pin_program_b.set_high().map_err(|_| Error::PinError)?;

        // INIT_B will go high at most 5ms after PROGRAM_B release.
        self.timer.sleep(Duration::from_millis(5));
        if self.pin_init_b.is_low().map_err(|_| Error::PinError)? {
            return Err(Error::ProgramError);
        }

        log::info!("FPGA is in program mode");
        let start_ms = self.timer.now_ms();

        let mut num_read = 0;
        while num_read < header.length {
            let amount = (header.length - num_read).min(scratch_buf.len());
            let buf = &mut scratch_buf[0..amount];
            bitstream
                .read_exact(buf)
                .map_err(|_| Error::BitstreamError)?;
            num_read += amount;

            self.program_spi
                .write(buf)
                .map_err(|_| Error::ProgramError)?;
        }

        log::info!(
            "Programmed FPGA, done={}, time={}",
            self.pin_done.is_high().map_err(|_| Error::PinError)?,
            self.timer.now_ms().saturating_sub(start_ms),
        );

        Ok(())
    }

    pub fn set_system_clock_rate(&mut self, rate: Hertz) {
        self.system_clock = rate;
    }

    pub fn enable_interrupt(&mut self, irq: Irq) -> Result<(), Error> {
        self.interrupts |= irq.as_flag();
        self.write_u32(REG_IRQ_ENABLE, self.interrupts)
    }

    pub fn disable_interrupt(&mut self, irq: Irq) -> Result<(), Error> {
        self.interrupts &= !irq.as_flag();
        self.write_u32(REG_IRQ_ENABLE, self.interrupts)
    }

    /// Finds a SPI data driver with the maximum clock speed.
    fn spi_transaction(
        &mut self,
        max_clock: Option<Hertz>,
        operations: &mut [Operation],
    ) -> Result<(), Error> {
        let driver = &mut self.data_spi.iter_mut().find(|(_, clock)| match max_clock {
            Some(max_clock) => *clock <= max_clock,
            None => true,
        });
        let driver = match driver {
            Some(driver) => driver,
            None => panic!("No suitable spi for max clock {:?}", max_clock),
        };
        driver
            .0
            .transaction(operations)
            .map_err(|_| Error::SpiError)
    }

    const fn spi_command(
        read: bool,
        word_size: FpgaSpiWordSize,
        byte_swap: bool,
        auto_increment: bool,
    ) -> u8 {
        (read as u8)
            | ((word_size as u8) << 1)
            | ((byte_swap as u8) << 3)
            | ((auto_increment as u8) << 4)
    }

    /// Generic SPI write function.
    pub fn spi_write(
        &mut self,
        max_clock: Option<Hertz>,
        command: SpiCommand,
        address: u32,
        data: &[u8],
    ) -> Result<(), Error> {
        let width = LineWidth::Quad;
        let mut command = command.as_write_command();
        command |= (width as u8) << 5;
        let address = address.to_be_bytes();
        self.spi_transaction(
            max_clock,
            &mut [
                Operation::Write(&[command]),
                Operation::WriteWithWidth(&address, width),
                Operation::WriteWithWidth(&data, width),
            ],
        )
    }

    /// Generic SPI read function.
    pub fn spi_read(
        &mut self,
        max_clock: Option<Hertz>,
        command: SpiCommand,
        address: u32,
        buffer: &mut [u8],
    ) -> Result<(), Error> {
        let width = LineWidth::Quad;
        let mut command = command.as_read_command();
        command |= (width as u8) << 5;
        let address = address.to_be_bytes();
        const DUMMY_BYTES: usize = 8;
        let mut dummy = [0u8; DUMMY_BYTES];
        self.spi_transaction(
            max_clock,
            &mut [
                Operation::Write(&[command]),
                Operation::WriteWithWidth(&address, width),
                Operation::ReadWithWidth(&mut dummy, width),
                Operation::ReadWithWidth(buffer, width),
            ],
        )
    }

    pub fn write_u16(&mut self, address: u32, data: u16) -> Result<(), Error> {
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits16,
            byte_swap: false,
            increment_address: true,
        };
        let data = data.to_be_bytes();
        self.spi_write(None, command, address, &data)
    }

    pub fn write_u32(&mut self, address: u32, data: u32) -> Result<(), Error> {
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits32,
            byte_swap: false,
            increment_address: true,
        };
        let data = data.to_be_bytes();
        self.spi_write(None, command, address, &data)
    }

    pub fn read_u32(&mut self, address: u32) -> Result<u32, Error> {
        let mut data = [0u8; 4];
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits32,
            byte_swap: false,
            increment_address: true,
        };
        self.spi_read(Some(MAX_SPI_READ_CLOCK), command, address, &mut data)?;
        Ok(u32::from_be_bytes(data))
    }

    pub fn sram_write(&mut self, address: u32, data: &[u8]) -> Result<(), Error> {
        let address = 0x0500_0000 | address;
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits16,
            byte_swap: true,
            increment_address: true,
        };
        // SRAM transfers at 16 bits per transfer and takes 3 (!) cycles.
        // (rate * (bits per transfer)) / ((bits per quad clock) * (cycles per transfer))
        let max_clock = (self.system_clock.0 * 16) / (4 * 3);
        self.spi_write(Some(Hertz(max_clock)), command, address, data)
    }

    pub fn sram_read(&mut self, address: u32, data: &mut [u8]) -> Result<(), Error> {
        let address = 0x0500_0000 | address;
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits16,
            byte_swap: true,
            increment_address: true,
        };
        let max_clock = ((self.system_clock.0 * 16) / (4 * 3)).min(MAX_SPI_READ_CLOCK.0);
        // log::info!("sram read with {:?}", max_clock);
        self.spi_read(Some(Hertz(max_clock)), command, address, data)
    }

    pub fn sdram_write(&mut self, address: u32, data: &[u8]) -> Result<(), Error> {
        let address = 0x8000_0000 | address;
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits32,
            byte_swap: true,
            increment_address: true,
        };
        // SDRAM transfers at 32 bits per transfer and takes 3.35 cycles on average (empirical).
        let max_clock = (self.system_clock.0 as f32) * (32.0 / 4.0) / 3.35;
        self.spi_write(Some(Hertz(max_clock as u32)), command, address, data)
    }

    pub fn sdram_read(&mut self, address: u32, data: &mut [u8]) -> Result<(), Error> {
        let address = 0x8000_0000 | address;
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits32,
            byte_swap: true,
            increment_address: true,
        };
        let max_clock =
            (((self.system_clock.0 as f32) * (32.0 / 4.0) / 3.35) as u32).min(MAX_SPI_READ_CLOCK.0);
        self.spi_read(Some(Hertz(max_clock)), command, address, data)
    }

    /// Configure the drawing bounds of the overlay.
    pub fn set_overlay_bounds(
        &mut self,
        start_x: u8,
        end_x: u8,
        scroll_x: u8,
        start_y: u8,
        end_y: u8,
        scroll_y: u8,
    ) -> Result<(), Error> {
        let config_x = ((start_x as u32) & 0xFF) << 16
            | ((end_x as u32) & 0xFF) << 8
            | ((scroll_x as u32) & 0xFF);
        let config_y = ((start_y as u32) & 0xFF) << 16
            | ((end_y as u32) & 0xFF) << 8
            | ((scroll_y as u32) & 0xFF);
        self.write_u32(REG_OVERLAY_XCTRL, config_x)?;
        self.write_u32(REG_OVERLAY_YCTRL, config_y)?;
        Ok(())
    }

    /// Hide the overlay by setting drawing bounds to invisible.
    pub fn hide_overlay(&mut self) -> Result<(), Error> {
        self.set_overlay_bounds(0, 0, 0, 0, 0, 0)
    }

    /// Write overlay framebuffer.
    pub fn write_overlay(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        let command = SpiCommand {
            word_size: FpgaSpiWordSize::Bits16,
            byte_swap: true,
            increment_address: true,
        };
        // 16 bits per transfer, 2 cycles per transfer.
        let max_clock = (self.system_clock.0 * 16) / (4 * 2);
        self.spi_write(Some(Hertz(max_clock)), command, 0x03000000 | offset, data)
    }

    /// Get the state of the cartridge slot button.
    pub fn get_cartridge_slot_button(&mut self) -> Result<bool, Error> {
        Ok((self.read_u32(REG_STATUS)? & 1) != 0)
    }

    pub fn set_display_mode(&mut self, new_mode: DisplayMode) -> Result<(), Error> {
        self.write_u32(REG_DISPLAY, (new_mode == DisplayMode::External) as u32)
    }
}

#[allow(unused)]
#[derive(Copy, Clone)]
pub enum FpgaSpiWordSize {
    Bits8 = 0,
    Bits16 = 1,
    Bits32 = 2,
    Bits64 = 3,
}

#[derive(Copy, Clone)]
pub struct SpiCommand {
    pub word_size: FpgaSpiWordSize,
    pub byte_swap: bool,
    pub increment_address: bool,
}

impl SpiCommand {
    fn as_read_command(self) -> u8 {
        (1u8)
            | ((self.word_size as u8) << 1)
            | ((self.byte_swap as u8) << 3)
            | ((self.increment_address as u8) << 4)
    }

    fn as_write_command(self) -> u8 {
        (0u8)
            | ((self.word_size as u8) << 1)
            | ((self.byte_swap as u8) << 3)
            | ((self.increment_address as u8) << 4)
    }
}
