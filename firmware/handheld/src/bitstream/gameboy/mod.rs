use esp_idf_svc::hal::units::Hertz;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::DerefMut,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use thiserror::Error;

use crate::{
    device::{drivers::fpga, Device},
    kvs, ui,
};

use super::{
    util::color_correction::{self, ColorCorrection},
    Bitstream,
};

mod dmg_palette;
mod rom;
mod rtc;

const SYSTEM_CLOCK_RATE: Hertz = Hertz(8 * 1024 * 1024);
const PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

const REG_EMU_CONFIG: u32 = 0xE000_0000;
const REG_EMU_CART_CONFIG: u32 = 0xE000_0004;
const REG_EMU_CART_ROM_ADDR: u32 = 0xE000_0008;
const REG_EMU_CART_ROM_MASK: u32 = 0xE000_000C;
const REG_EMU_CART_RAM_ADDR: u32 = 0xE000_0010;
const REG_EMU_CART_RAM_MASK: u32 = 0xE000_0014;
const REG_RTC_STATE: u32 = 0xE000_0018;
const REG_RTC_LATCHED: u32 = 0xE000_001C;
const REG_IMU_ACCEL_X: u32 = 0xE000_0020;
const REG_IMU_ACCEL_Y: u32 = 0xE000_0024;
const REG_DMG_PALETTE_OFF: u32 = 0xE000_0030;
const REG_SGB_BUTTONS: u32 = 0xE000_0034;
const REG_STAT_STALLS: u32 = 0xE000_1000;
const REG_STAT_CYCLES: u32 = 0xE000_1004;
const BIOS_ADDRESS_BASE: u32 = 0xE010_0000;
const DMG_PALETTE_BASE: u32 = 0xE020_0000;

#[derive(Debug, Error)]
pub enum GameboyError {
    #[error("unsupported cartridge type {0}")]
    UnsupportedCartridgeType(u8),
    #[error("I/O error")]
    IoError(#[from] std::io::Error),
    #[error("FPGA error")]
    FpgaError(#[from] crate::device::drivers::fpga::Error),
    #[error("Invalid bootrom")]
    InvalidBootrom,
}

/// Driver for Gameboy FPGA module
pub struct Gameboy {
    /// Rom header, if this is an emulated cartridge
    rom_header: Option<rom::RomHeader>,
    /// Path to the RAM file, if this is an emulated cartridge.
    ram_path: Option<PathBuf>,
    /// Loaded bootrom path.
    bootrom_path: Option<&'static str>,
}

impl Gameboy {
    pub fn new() -> Self {
        Gameboy {
            rom_header: None,
            ram_path: None,
            bootrom_path: None,
        }
    }

    fn get_bootrom_path() -> &'static str {
        let is_dmg = kvs::keys::GB_IS_DMG.get().unwrap();
        let skip = kvs::keys::GB_SKIP_BOOT_ANIM.get().unwrap();

        if is_dmg {
            if skip {
                "gameboy.bios-dmg-fast.bin"
            } else {
                "gameboy.bios-dmg.bin"
            }
        } else {
            if skip {
                "gameboy.bios-cgb-fast.bin"
            } else {
                "gameboy.bios-cgb.bin"
            }
        }
    }

    fn load_bootrom(&mut self, device: &mut Device) -> Result<(), GameboyError> {
        let bios_path = Self::get_bootrom_path();
        if self.bootrom_path == Some(bios_path) {
            return Ok(());
        }

        log::info!("Loading CGB bootrom");
        let mut bios_file = crate::util::open_system_file(bios_path)?;
        let mut scratch = super::SCRATCH.take().expect("scratch buffer");
        let mut buf = &mut scratch[..2048];

        let file_len = bios_file.metadata()?.len();
        if file_len == 2048 || file_len == 256 {
            bios_file.read(&mut buf)?;
        } else if file_len == (2048 + 256) {
            // Assume this is a bootrom with 256 bytes of padding at offset 256.
            log::warn!("Removing CGB bootrom padding");
            bios_file.read(&mut buf[0..256])?;
            bios_file.seek(SeekFrom::Current(256))?;
            bios_file.read(&mut buf[256..])?;
        } else {
            log::error!("Bootrom invalid length: {}", file_len);
            return Err(GameboyError::InvalidBootrom);
        }

        let address = BIOS_ADDRESS_BASE;
        let command = fpga::SpiCommand {
            word_size: fpga::FpgaSpiWordSize::Bits8,
            byte_swap: true,
            increment_address: true,
        };
        // 8 bits per transfer, 2 clocks each.
        // This would be ~8 MHz. However, since it's such a short transfer, we can do a slightly
        // higher rate and let the SPI FIFO buffer it.
        let max_clock = Hertz(10_000_000);
        device
            .fpga
            .spi_write(Some(max_clock), command, address, &buf)?;

        self.bootrom_path = Some(bios_path);
        Ok(())
    }

    /// Prepare to load a new cartridge (physical or emulated)
    fn initialize(&mut self, device: &mut Device) -> Result<(), GameboyError> {
        // Hold in reset
        device.fpga.write_u32(fpga::REG_CONTROL, 0b0000)?;

        // Set configuration
        let is_dmg = kvs::keys::GB_IS_DMG.get().unwrap();
        let config = 0 | (((!is_dmg) as u32) << 0);
        device.fpga.write_u32(REG_EMU_CONFIG, config)?;
        device.fpga.write_u32(
            REG_SGB_BUTTONS,
            kvs::keys::GB_SGB_BUTTONS.get().unwrap() as u32,
        )?;

        device.imu.disable_accel().unwrap();

        // Disable vblank IRQ
        device.fpga.disable_interrupt(fpga::Irq::ModuleVblank)?;

        // Color correction
        let correction: &ColorCorrection = {
            use color_correction::presets::*;
            let corrections = [&IDENTITY, &GBC_GBA, &GBC_GBA, &GBA_AGS101];
            if is_dmg {
                &IDENTITY
            } else {
                let setting = kvs::keys::CGB_COLOR_PROFILE.get().unwrap() as usize;
                corrections.get(setting).unwrap_or(&&IDENTITY)
            }
        };
        // TODO: only configure if it has changed
        correction.configure(device)?;

        // DMG palettes
        if is_dmg {
            let setting = kvs::keys::DMG_COLOR_PALETTE.get().unwrap() as usize;
            let palette = dmg_palette::PALETTES
                .get(setting)
                .unwrap_or(&dmg_palette::PALETTES[0]);
            palette.load(device)?;
        }

        // Bootrom
        self.load_bootrom(device)?;

        Ok(())
    }

    pub fn set_physical_cartridge(&mut self) -> Result<(), GameboyError> {
        self.ram_path = None;

        let mut device = Device::lock();
        self.initialize(&mut device)?;

        // Switch to physical cartridge.
        device.fpga.write_u32(REG_EMU_CART_CONFIG, 0)?;

        // Resume
        device.fpga.write_u32(fpga::REG_CONTROL, 0b1011)?;
        device.imu.disable_accel().unwrap();

        Ok(())
    }

    pub fn set_emulated_cartridge(&mut self, rom_path: &Path) -> Result<(), GameboyError> {
        {
            let mut device = Device::lock();
            self.initialize(&mut device)?;
        }

        // Load ROM
        let mut rom_file = File::open(rom_path)?;
        let rom_file_size = rom_file.metadata()?.len() as u32;
        let mut rom_header = [0u8; 0x150];
        rom_file.read(&mut rom_header)?;
        let rom_header = rom::RomHeader::parse(rom_header)?;
        rom_file.seek(std::io::SeekFrom::Start(0))?;
        log::info!("Loading rom: {:?}", rom_header);

        let mut scratch = super::SCRATCH.take().expect("scratch buffer");
        let mut last_progress_update = Instant::now();
        let mut total = 0u32;
        crate::util::background_io::iter_chunks(rom_file, &mut scratch, |chunk| {
            let _ = Device::lock().fpga.sdram_write(total, &chunk);
            total += chunk.len() as u32;

            // Update UI progress bar.
            if last_progress_update.elapsed() > PROGRESS_UPDATE_INTERVAL {
                let progress = (total as f32) / (rom_file_size as f32);
                ui::send(ui::Message::RomLoadingProgress(progress));
                last_progress_update = Instant::now();
            }
        })?;
        ui::send(ui::Message::RomLoadingProgress(1.0));
        drop(scratch);

        // Load RAM
        let ram_path = rom_path.with_extension("sav");
        let _ = crate::util::copy_file(&ram_path, &ram_path.with_extension("sav.bak"));
        match File::open(ram_path.as_path()) {
            Ok(mut ram_file) => {
                log::info!("Loading RAM");
                let mut scratch = super::SCRATCH.take().expect("scratch buffer");
                let buf = scratch.deref_mut();

                let mut pos = 0u32;
                while pos < rom_header.ram_size {
                    let to_read = ((rom_header.ram_size - pos) as usize).min(buf.len());
                    let n = ram_file.read(&mut buf[..to_read])?;
                    if n == 0 {
                        break;
                    }
                    Device::lock().fpga.sram_write(pos, &buf[..n])?;
                    pos += n as u32;
                }

                if rom_header.has_rtc {
                    // Read next 48 bytes for RTC data.
                    let n = ram_file.read(&mut buf[..48])?;
                    if n == 48 {
                        let mut rtc_state =
                            rtc::RtcState::from_disk(&buf[0..20].try_into().unwrap());
                        let rtc_latched =
                            rtc::RtcState::from_disk(&buf[20..40].try_into().unwrap());
                        let rtc_timestamp = u64::from_le_bytes(buf[40..48].try_into().unwrap());
                        let mut device = Device::lock();
                        let elapsed = device
                            .get_datetime()
                            .unix_timestamp()
                            .saturating_sub_unsigned(rtc_timestamp);
                        rtc_state.advance(elapsed as u64);
                        device.fpga.write_u32(REG_RTC_STATE, rtc_state.to_fpga())?;
                        device
                            .fpga
                            .write_u32(REG_RTC_LATCHED, rtc_latched.to_fpga())?;
                        log::info!(
                            "Loaded saved RTC state: {:?}, elapsed={}",
                            rtc_state,
                            elapsed
                        );
                    }
                }
            }
            Err(_) => {
                log::info!("Not loading RAM");
            }
        }

        let mut device = Device::lock();

        // Configure emulated cartridge control registers
        device
            .fpga
            .write_u32(REG_EMU_CART_CONFIG, rom_header.as_emu_cart_config())?;
        device.fpga.write_u32(REG_EMU_CART_ROM_ADDR, 0)?;
        device
            .fpga
            .write_u32(REG_EMU_CART_ROM_MASK, rom_header.rom_size - 1)?;
        device.fpga.write_u32(REG_EMU_CART_RAM_ADDR, 0)?;
        device
            .fpga
            .write_u32(REG_EMU_CART_RAM_MASK, rom_header.ram_size - 1)?;

        // If IMU is needed, enable vsync IRQ
        if rom_header.has_sensor {
            // XXX: if other components need IMU too, switch to a global lease system
            device.imu.enable_accel().unwrap();
            device.fpga.enable_interrupt(fpga::Irq::ModuleVblank)?;
        }

        // Resume
        device.fpga.write_u32(fpga::REG_CONTROL, 0b1011)?;

        self.ram_path = Some(ram_path);
        self.rom_header = Some(rom_header);
        Ok(())
    }

    /// Persists the game save RAM to disk, if using an emulated cartridge.
    pub fn persist_ram(&mut self) -> Result<(), GameboyError> {
        let ram_path = match self.ram_path.as_ref() {
            Some(ram_path) => ram_path,
            None => return Ok(()),
        };

        let ram_size = self.rom_header.as_ref().map_or(0, |h| h.ram_size);
        log::info!("Saving RAM: {}", ram_path.display());

        let mut file = File::create(ram_path)?;
        let mut scratch = super::SCRATCH.take().expect("scratch buffer");
        let buf = scratch.deref_mut();
        let mut address: u32 = 0;
        let mut bytes_left = ram_size as usize;

        let mut device = Device::lock();
        while bytes_left > 0 {
            let to_read = bytes_left.min(buf.len());
            let data = &mut buf[0..to_read];
            device.fpga.sram_read(address, data)?;
            file.write(data)?;
            address += to_read as u32;
            bytes_left -= to_read;
        }

        if self.rom_header.as_ref().map_or(false, |h| h.has_rtc) {
            let rtc_state = rtc::RtcState::from_fpga(device.fpga.read_u32(REG_RTC_STATE)?);
            let rtc_latched = rtc::RtcState::from_fpga(device.fpga.read_u32(REG_RTC_LATCHED)?);
            file.write(&rtc_state.to_disk())?;
            file.write(&rtc_latched.to_disk())?;
            file.write(&(device.get_datetime().unix_timestamp() as u64).to_le_bytes())?;
            log::info!("Wrote RTC state: {:?}", rtc_state);
        }

        Ok(())
    }

    /// Return whether the current save game would need to be persisted to disk.
    pub fn needs_save_persist(&self) -> bool {
        self.ram_path.is_some()
    }
}

impl Bitstream for Gameboy {
    fn get_bitstream_path(&self) -> &'static str {
        return "gameboy.bit.hs";
    }

    fn on_after_program(&mut self) -> Result<(), String> {
        Device::lock().fpga.set_system_clock_rate(SYSTEM_CLOCK_RATE);
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), fpga::Error> {
        let mut device = Device::lock();

        // Enable/disable IMU as needed
        if !paused && self.rom_header.as_ref().map_or(false, |h| h.has_sensor) {
            device.imu.enable_accel().unwrap();
        } else {
            device.imu.disable_accel().unwrap();
        }

        device
            .fpga
            .write_u32(fpga::REG_CONTROL, 0b1010u32 | ((!paused) as u32))?;

        if paused {
            // Debug output stall stats
            let num_cycles = device.fpga.read_u32(REG_STAT_CYCLES)?;
            let num_stalls = device.fpga.read_u32(REG_STAT_STALLS)?;
            device.fpga.write_u32(REG_STAT_CYCLES, 0)?;
            device.fpga.write_u32(REG_STAT_STALLS, 0)?;
            let rate = (num_cycles as f32) / ((num_cycles as f32) + (num_stalls as f32));
            log::info!("Run rate: {}%", rate * 100.0);
        }

        Ok(())
    }

    fn reset(&mut self) -> Result<(), fpga::Error> {
        let mut device = Device::lock();
        device.fpga.write_u32(fpga::REG_CONTROL, 0b0000)?;
        device.fpga.write_u32(fpga::REG_CONTROL, 0b1010)?;
        Ok(())
    }

    fn on_vblank_irq(&mut self) {
        let mut device = Device::lock();
        let sample = device.imu.read_accel().unwrap();
        // Invert X and Y
        let accel_x = ((0x81D0 as f32) + ((0x70 as f32) * -sample.x)) as u16;
        let accel_y = ((0x81D0 as f32) + ((0x70 as f32) * -sample.y)) as u16;
        device
            .fpga
            .write_u32(REG_IMU_ACCEL_X, accel_x as u32)
            .unwrap();
        device
            .fpga
            .write_u32(REG_IMU_ACCEL_Y, accel_y as u32)
            .unwrap();
    }
}
