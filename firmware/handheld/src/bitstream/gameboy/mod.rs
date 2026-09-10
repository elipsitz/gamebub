use esp_idf_svc::hal::units::Hertz;
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::PathBuf,
};
use thiserror::Error;

use crate::{
    core::{
        CoreCartridgeMode, CoreFile, CoreHandler, CoreInfo, CoreSetting, CoreSettingListItem,
        CoreSettingType,
    },
    device::{drivers::fpga, Device},
    kvs,
};

use super::{util::color_correction, Bitstream};

mod dmg_palette;
mod rom;
mod rtc;

const SYSTEM_CLOCK_RATE: Hertz = Hertz(8 * 1024 * 1024);

const REG_EMU_CONFIG: u32 = 0x0000_0000;
const REG_EMU_CART_CONFIG: u32 = 0x0000_0004;
const REG_EMU_CART_ROM_ADDR: u32 = 0x0000_0008;
const REG_EMU_CART_ROM_MASK: u32 = 0x0000_000C;
const REG_EMU_CART_RAM_ADDR: u32 = 0x0000_0010;
const REG_EMU_CART_RAM_MASK: u32 = 0x0000_0014;
const REG_RTC_STATE: u32 = 0x0000_0018;
const REG_RTC_LATCHED: u32 = 0x0000_001C;
const REG_IMU_ACCEL_X: u32 = 0x0000_0020;
const REG_IMU_ACCEL_Y: u32 = 0x0000_0024;
const REG_DMG_PALETTE_OFF: u32 = 0x0000_0030;
const REG_STAT_STALLS: u32 = 0x0000_1000;
const REG_STAT_CYCLES: u32 = 0x0000_1004;
const REG_RESET_ONCE: u32 = 0x0000_2000;
const DMG_PALETTE_BASE: u32 = 0x2000_0000;
const COLOR_CORRECTION_BASE: u32 = 0x5000_0000;

const FILE_ROM: u16 = 0;
const FILE_SAVE: u16 = 1;
const FILE_BIOS_CGB: u16 = 2;
const FILE_BIOS_DMG: u16 = 3;

const SETTING_RESET: u16 = 0;
const SETTING_GB_MODE: u16 = 1;
const SETTING_GBC_COLOR_CORRECTIONS: u16 = 2;
const SETTING_GB_COLOR_PALETTE: u16 = 3;

#[derive(Debug, Error)]
pub enum GameboyError {
    #[error("unsupported cartridge type {0}")]
    UnsupportedCartridgeType(u8),
    #[error("I/O error")]
    IoError(#[from] std::io::Error),
    #[error("FPGA error")]
    FpgaError(#[from] crate::device::drivers::fpga::Error),
}

/// Driver for Gameboy FPGA module
pub struct Gameboy {
    /// Rom header, if this is an emulated cartridge
    rom_header: Option<rom::RomHeader>,
    /// Size of the ROM
    rom_file_size: u32,
    /// Path to the RAM file, if this is an emulated cartridge.
    ram_path: Option<PathBuf>,

    /// RTC state loaded from the RAM file
    rtc_state: Option<(rtc::RtcState, rtc::RtcState)>,
}

impl Gameboy {
    pub fn new() -> Self {
        Gameboy {
            rom_header: None,
            rom_file_size: 0,
            ram_path: None,
            rtc_state: None,
        }
    }

    /// Prepare to load a new cartridge (physical or emulated)
    fn initialize(&mut self, device: &mut Device) -> Result<(), GameboyError> {
        device.imu.disable_accel().unwrap();

        // Disable vblank IRQ
        device.fpga.disable_interrupt(fpga::Irq::ModuleVblank)?;

        Ok(())
    }

    pub fn get_core_info() -> CoreInfo {
        CoreInfo {
            id: "Game-Bub.GB".try_into().unwrap(),
            name: "Game Boy / Game Boy Color".try_into().unwrap(),
            author: "Game Bub".try_into().unwrap(),
            is_built_in: true,
            core_dir: PathBuf::new(),
            uses_cartridge: CoreCartridgeMode::IfSelected,
            files: [
                CoreFile {
                    id: SETTING_RESET,
                    label: "ROM".try_into().unwrap(),
                    extensions: ["gb".try_into().unwrap(), "gbc".try_into().unwrap()]
                        .into_iter()
                        .collect(),
                    filename: None,

                    optional: true,
                    read_only: true,
                    user_selected: true,
                    dependent_on_0: false,
                    initialize: false,

                    address: 0x3000_0000, // SDRAM
                    max_size: 8 * 1024 * 1024,
                    exact_size: 0,
                    max_transfer_speed: 10_000, // 10 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits32,
                },
                CoreFile {
                    id: 1,
                    label: "Save".try_into().unwrap(),
                    extensions: ["sav".try_into().unwrap()].into_iter().collect(),
                    filename: None,

                    optional: true,
                    read_only: false,
                    user_selected: false,
                    dependent_on_0: true,
                    initialize: true,

                    address: 0x4000_0000, // SRAM
                    max_size: 128 * 1024 + 48,
                    exact_size: 0,
                    max_transfer_speed: 5_000, // 5 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits16,
                },
                CoreFile {
                    id: 2,
                    label: "BIOS CGB".try_into().unwrap(),
                    extensions: ["bin".try_into().unwrap()].into_iter().collect(),
                    filename: None, // TODO

                    optional: false,
                    read_only: true,
                    user_selected: false,
                    dependent_on_0: false,
                    initialize: false,

                    address: 0x1000_0000 + 256,
                    max_size: 0,
                    exact_size: 2048 + 256,
                    max_transfer_speed: 5_000, // 5 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits8,
                },
                CoreFile {
                    id: 3,
                    label: "BIOS DMG".try_into().unwrap(),
                    extensions: ["bin".try_into().unwrap()].into_iter().collect(),
                    filename: None, // TODO

                    optional: false,
                    read_only: true,
                    user_selected: false,
                    dependent_on_0: false,
                    initialize: false,

                    address: 0x1000_0000,
                    max_size: 0,
                    exact_size: 256,
                    max_transfer_speed: 5_000, // 5 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits8,
                },
            ]
            .into_iter()
            .collect(),
            settings: [
                CoreSetting {
                    id: 0,
                    label: "Reset Core".into(),
                    address: REG_RESET_ONCE,
                    mask: 0,
                    default: 0,
                    inner: CoreSettingType::Action { value: 1 },
                },
                CoreSetting {
                    id: SETTING_GB_MODE,
                    label: "Enable GB Mode".into(),
                    address: 0xFFFF_FFFF,
                    mask: 0,
                    default: 0,
                    inner: CoreSettingType::Checkbox { value: 1 },
                },
                CoreSetting {
                    id: SETTING_GBC_COLOR_CORRECTIONS,
                    label: "GBC Color Corrections".into(),
                    address: 0xFFFF_FFFF,
                    mask: 0,
                    default: 1,
                    inner: CoreSettingType::List {
                        items: ["None", "GBC", "GBA", "GBA SP"]
                            .iter()
                            .enumerate()
                            .map(|(i, &x)| CoreSettingListItem {
                                value: i as u32,
                                label: x.into(),
                            })
                            .collect(),
                    },
                },
                CoreSetting {
                    id: SETTING_GB_COLOR_PALETTE,
                    label: "GB Color Palette".into(),
                    address: 0xFFFF_FFFF,
                    mask: 0,
                    default: 1,
                    inner: CoreSettingType::List {
                        items: ["Grayscale", "DMG Green", "GB Pocket"]
                            .iter()
                            .enumerate()
                            .map(|(i, &x)| CoreSettingListItem {
                                value: i as u32,
                                label: x.into(),
                            })
                            .collect(),
                    },
                },
            ]
            .into_iter()
            .collect(),
            bitstream: crate::util::get_system_file_path("gameboy.bit.hs"),
        }
    }
}

impl Bitstream for Gameboy {
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

impl CoreHandler for Gameboy {
    fn as_legacy_bitstream(&mut self) -> &mut dyn super::Bitstream {
        self
    }

    fn on_after_program(&mut self) {
        Device::lock().fpga.set_system_clock_rate(SYSTEM_CLOCK_RATE);
    }

    fn get_file_path_override(&mut self, id: u16) -> Option<PathBuf> {
        match id {
            FILE_BIOS_CGB => Some(crate::util::get_system_file_path("gameboy.bios-cgb.bin")),
            FILE_BIOS_DMG => Some(crate::util::get_system_file_path("gameboy.bios-dmg.bin")),
            _ => None,
        }
    }

    fn on_before_file_load(&mut self, id: u16, file: &mut File) -> Result<(), String> {
        if id == FILE_ROM {
            self.rom_file_size = file.metadata().map_err(|_| "I/O")?.len() as u32;
            let mut rom_header = [0u8; 0x150];
            file.read(&mut rom_header).map_err(|_| "I/O")?;
            let rom_header = rom::RomHeader::parse(rom_header).map_err(|e| e.to_string())?;
            file.seek(std::io::SeekFrom::Start(0)).map_err(|_| "I/O")?;
            self.rom_header = Some(rom_header);
        } else if id == FILE_SAVE {
            let rom_header = self.rom_header.as_ref().unwrap();
            if rom_header.has_rtc {
                // Read next 48 bytes for RTC data.
                file.seek(std::io::SeekFrom::Start(rom_header.ram_size as u64))
                    .map_err(|_| "I/O")?;
                let mut buf = [0u8; 48];
                let n = file.read(&mut buf).map_err(|_| "I/O")?;
                if n == 48 {
                    let mut rtc_state = rtc::RtcState::from_disk(&buf[0..20].try_into().unwrap());
                    let rtc_latched = rtc::RtcState::from_disk(&buf[20..40].try_into().unwrap());
                    let rtc_timestamp = u64::from_le_bytes(buf[40..48].try_into().unwrap());
                    let mut device = Device::lock();
                    let elapsed = device
                        .get_datetime()
                        .unix_timestamp()
                        .saturating_sub_unsigned(rtc_timestamp);
                    rtc_state.advance(elapsed as u64);
                    log::info!(
                        "Loaded saved RTC state: {:?}, elapsed={}",
                        rtc_state,
                        elapsed
                    );
                    self.rtc_state = Some((rtc_state, rtc_latched));
                }
                file.seek(std::io::SeekFrom::Start(0)).map_err(|_| "I/O")?;
            }
        }
        Ok(())
    }

    fn on_before_run(&mut self) -> Result<(), String> {
        self.ram_path = None;

        let mut device = Device::lock();
        self.initialize(&mut device).map_err(|e| e.to_string())?;

        if let Some(rom_header) = self.rom_header.as_ref() {
            // Configure RTC if needed
            if let Some((rtc_state, rtc_latched)) = self.rtc_state {
                let _ = device.fpga.write_u32(REG_RTC_STATE, rtc_state.to_fpga());
                let _ = device
                    .fpga
                    .write_u32(REG_RTC_LATCHED, rtc_latched.to_fpga());
            }

            // Configure emulated cartridge control registers
            let _ = device
                .fpga
                .write_u32(REG_EMU_CART_CONFIG, rom_header.as_emu_cart_config());
            let _ = device.fpga.write_u32(REG_EMU_CART_ROM_ADDR, 0);
            let _ = device
                .fpga
                .write_u32(REG_EMU_CART_ROM_MASK, rom_header.rom_size - 1);
            let _ = device.fpga.write_u32(REG_EMU_CART_RAM_ADDR, 0);
            let _ = device
                .fpga
                .write_u32(REG_EMU_CART_RAM_MASK, rom_header.ram_size - 1);

            // If IMU is needed, enable vsync IRQ
            if rom_header.has_sensor {
                // XXX: if other components need IMU too, switch to a global lease system
                device.imu.enable_accel().unwrap();
                device
                    .fpga
                    .enable_interrupt(fpga::Irq::ModuleVblank)
                    .unwrap();
            }
        } else {
            // Switch to physical cartridge.
            let _ = device.fpga.write_u32(REG_EMU_CART_CONFIG, 0);
        }

        Ok(())
    }

    fn get_file_size(&mut self, id: u16) -> Option<u32> {
        assert!(id == FILE_SAVE);
        Some(self.rom_header.as_ref().map_or(0, |h| h.ram_size))
    }

    fn on_after_file_save(&mut self, id: u16, file: &mut File) -> Result<(), String> {
        assert!(id == FILE_SAVE);

        // Save RTC
        if self.rom_header.as_ref().map_or(false, |h| h.has_rtc) {
            use rtc::RtcState;
            let mut device = Device::lock();
            let rtc_state = RtcState::from_fpga(device.fpga.read_u32(REG_RTC_STATE).unwrap());
            let rtc_latched = RtcState::from_fpga(device.fpga.read_u32(REG_RTC_LATCHED).unwrap());
            let timestamp = device.get_datetime().unix_timestamp() as u64;

            file.write(&rtc_state.to_disk()).map_err(|_| "I/O")?;
            file.write(&rtc_latched.to_disk()).map_err(|_| "I/O")?;
            file.write(&timestamp.to_le_bytes()).map_err(|_| "I/O")?;
            log::info!("Wrote RTC state: {:?}", rtc_state);
        }
        Ok(())
    }

    fn on_focus_changed(&mut self, has_focus: bool) {
        let paused = !has_focus;
        let mut device = Device::lock();

        // Enable/disable IMU as needed
        if self.rom_header.as_ref().map_or(false, |h| h.has_sensor) {
            if paused {
                device.imu.disable_accel().unwrap();
            } else {
                device.imu.enable_accel().unwrap();
            }
        }

        if paused {
            // Debug output stall stats
            let num_cycles = device.fpga.read_u32(REG_STAT_CYCLES).unwrap();
            let num_stalls = device.fpga.read_u32(REG_STAT_STALLS).unwrap();
            let _ = device.fpga.write_u32(REG_STAT_CYCLES, 0);
            let _ = device.fpga.write_u32(REG_STAT_STALLS, 0);
            let rate = (num_cycles as f32) / ((num_cycles as f32) + (num_stalls as f32));
            log::info!("Run rate: {}%", rate * 100.0);
        }
    }

    fn load_settings(&mut self) -> Vec<(u16, u32)> {
        vec![
            (SETTING_GB_MODE, kvs::keys::GB_IS_DMG.get().unwrap() as u32),
            (
                SETTING_GBC_COLOR_CORRECTIONS,
                kvs::keys::CGB_COLOR_PROFILE.get().unwrap() as u32,
            ),
            (
                SETTING_GB_COLOR_PALETTE,
                kvs::keys::DMG_COLOR_PALETTE.get().unwrap() as u32,
            ),
        ]
    }

    fn on_setting_changed(&mut self, id: u16, value: u32) {
        let mut device = Device::lock();
        match id {
            SETTING_GB_MODE => {
                let is_dmg = value == 1;
                let config = 0 | (((!is_dmg) as u32) << 0);
                let _ = device.fpga.write_u32(REG_EMU_CONFIG, config);
                let _ = device.fpga.write_u32(REG_RESET_ONCE, 1);
                kvs::keys::GB_IS_DMG.set(&is_dmg);
            }
            SETTING_GBC_COLOR_CORRECTIONS => {
                kvs::keys::CGB_COLOR_PROFILE.set(&(value as i32));
                let correction = {
                    use color_correction::presets::*;
                    let corrections = [&IDENTITY, &GBC_GBA, &GBC_GBA, &GBA_AGS101];
                    if kvs::keys::GB_IS_DMG.get().unwrap_or_default() {
                        &IDENTITY
                    } else {
                        corrections.get(value as usize).unwrap_or(&&IDENTITY)
                    }
                };
                let _ = correction.configure(&mut device, COLOR_CORRECTION_BASE);
            }
            SETTING_GB_COLOR_PALETTE => {
                let palette = dmg_palette::PALETTES
                    .get(value as usize)
                    .unwrap_or(&dmg_palette::PALETTES[0]);
                let _ = palette.load(&mut device);
            }
            _ => {}
        }
    }
}
