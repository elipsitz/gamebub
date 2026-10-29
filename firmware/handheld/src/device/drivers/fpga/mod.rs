#![allow(dead_code)]

use std::{
    io::Read,
    time::{Duration, Instant},
};

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

pub const REG_INFO_FRAMEWORK_VER: u32 = 0xF100_0000;
pub const REG_INFO_SYSCLK_HZ: u32 = 0xF100_0004;
pub const REG_INFO_VIDEO_DIM: u32 = 0xF100_0100;
pub const REG_INFO_VIDEO_DEPTH: u32 = 0xF100_0104;

pub const REG_CTRL_IRQ_ENABLE: u32 = 0xF100_1000;
pub const REG_CTRL_IRQ_PENDING: u32 = 0xF100_1004;
pub const REG_CTRL_BUTTON_FORCE: u32 = 0xF100_1008;
pub const REG_CTRL_DOCK: u32 = 0xF100_100C;
pub const REG_CTRL_FOCUS: u32 = 0xF100_1010;
pub const REG_CTRL_VIBRATE: u32 = 0xF100_1014;

pub const REG_CTRL_CMD_HOST: u32 = 0xF100_1100;
pub const REG_CTRL_CMD_CORE: u32 = 0xF100_1104;

pub const REG_STATUS_BUTTON: u32 = 0xF100_2000;
pub const REG_STATUS_CART_SWITCH: u32 = 0xF100_2004;

pub const REG_CMD_HOST_BASE: u32 = 0xF000_0000;
pub const REG_CMD_CORE_BASE: u32 = 0xF000_1000;

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
}

impl<'a, PinDone, PinProgramB, PinInitB, ProgramSpi>
    Fpga<'a, PinDone, PinProgramB, PinInitB, ProgramSpi>
where
    PinDone: InputPin,
    PinProgramB: OutputPin,
    PinInitB: InputPin,
    ProgramSpi: SpiDevice,
{
    pub fn new(
        pin_done: PinDone,
        pin_program_b: PinProgramB,
        pin_init_b: PinInitB,
        data_spi: Vec<(SpiDataDriver<'a>, Hertz)>,
        program_spi: ProgramSpi,
    ) -> Self {
        Fpga {
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
    pub fn program(
        &mut self,
        bitstream: &mut dyn Read,
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
        let start_time = Instant::now();
        while self.pin_init_b.is_low().map_err(|_| Error::PinError)? {
            if start_time.elapsed() > Duration::from_millis(35) {
                return Err(Error::ProgramError);
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Pull PROGRAM_B low, hold it for at least 250ns.
        self.pin_program_b.set_low().map_err(|_| Error::PinError)?;
        std::thread::sleep(Duration::from_millis(1));
        if self.pin_init_b.is_high().map_err(|_| Error::PinError)? {
            return Err(Error::ProgramError);
        }
        self.pin_program_b.set_high().map_err(|_| Error::PinError)?;

        // INIT_B will go high at most 5ms after PROGRAM_B release.
        std::thread::sleep(Duration::from_millis(5));
        if self.pin_init_b.is_low().map_err(|_| Error::PinError)? {
            return Err(Error::ProgramError);
        }

        log::info!("FPGA is in program mode");
        let start_time = Instant::now();

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
            start_time.elapsed().as_millis() as u32,
        );

        Ok(())
    }

    pub fn set_system_clock_rate(&mut self, rate: Hertz) {
        self.system_clock = rate;
    }

    pub fn enable_interrupt(&mut self, irq: Irq) -> Result<(), Error> {
        self.interrupts |= irq.as_flag();
        self.write_u32(REG_CTRL_IRQ_ENABLE, self.interrupts)
    }

    pub fn disable_interrupt(&mut self, irq: Irq) -> Result<(), Error> {
        self.interrupts &= !irq.as_flag();
        self.write_u32(REG_CTRL_IRQ_ENABLE, self.interrupts)
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
                Operation::WriteWithWidth(data, width),
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

    pub fn write_u32(&mut self, address: u32, data: u32) -> Result<(), Error> {
        let command = SpiCommand::new(FpgaSpiWordSize::Bits32);
        let data = data.to_le_bytes();
        self.spi_write(None, command, address, &data)
    }

    pub fn read_u32(&mut self, address: u32) -> Result<u32, Error> {
        let mut data = [0u8; 4];
        let command = SpiCommand::new(FpgaSpiWordSize::Bits32);
        self.spi_read(Some(MAX_SPI_READ_CLOCK), command, address, &mut data)?;
        Ok(u32::from_le_bytes(data))
    }

    /// Write overlay framebuffer.
    pub fn write_overlay(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        let command = SpiCommand::new(FpgaSpiWordSize::Bits16);
        // 16 bits per transfer, 2 cycles per transfer.
        let max_clock = (self.system_clock.0 * 16) / (4 * 2);
        self.spi_write(Some(Hertz(max_clock)), command, 0xF200_0000 | offset, data)
    }

    /// Get the state of the cartridge slot button.
    pub fn get_cartridge_slot_button(&mut self) -> Result<bool, Error> {
        Ok((self.read_u32(REG_STATUS_CART_SWITCH)? & 1) != 0)
    }

    pub fn set_display_mode(&mut self, new_mode: DisplayMode) -> Result<(), Error> {
        self.write_u32(REG_CTRL_DOCK, (new_mode == DisplayMode::External) as u32)
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
    pub fn new(word_size: FpgaSpiWordSize) -> Self {
        SpiCommand {
            word_size,
            byte_swap: true,
            increment_address: true,
        }
    }

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
