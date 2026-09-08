use std::{
    fmt::{Debug, Display},
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use esp_idf_svc::hal::units::Hertz;
use rtc::RtcState;
use thiserror::Error;

use crate::{
    core::{CoreFile, CoreHandler, CoreInfo, CoreSetting, CoreSettingListItem, CoreSettingType},
    device::{drivers::fpga, Device},
    kvs, ui,
};

use super::{
    util::color_correction::{self, ColorCorrection},
    Bitstream,
};
use save_type_detector::SaveTypeDetector;

mod game_db;
mod rtc;
mod save_type_detector;

const SYSTEM_CLOCK_RATE: Hertz = Hertz(16 * 1024 * 1024);
const ROM_HEADER_LENGTH: usize = 192;

const REG_EMU_CART_CONFIG: u32 = 0x0000_0000;
const REG_EMU_CART_ROM_SIZE: u32 = 0x0000_0004;
const REG_GB_PLAYER: u32 = 0x0000_0008;
const REG_IMU_GYRO_Z: u32 = 0x0000_0100;
const REG_IMU_ACCEL_X: u32 = 0x0000_0104;
const REG_IMU_ACCEL_Y: u32 = 0x0000_0108;
const REG_RTC_LO: u32 = 0x0000_0200;
const REG_RTC_HI: u32 = 0x0000_0204;
const REG_STAT_STALLS: u32 = 0x0000_1000;
const REG_STAT_CYCLES: u32 = 0x0000_1004;
const COLOR_CORRECTION_BASE: u32 = 0x5000_0000;

const FILE_ROM: u16 = 0;
const FILE_SAVE: u16 = 1;
const FILE_BIOS: u16 = 2;

const SETTING_RESET: u16 = 0;
const SETTING_COLOR_CORRECTIONS: u16 = 1;
const SETTING_GAME_BOY_PLAYER: u16 = 2;

#[derive(Debug, Error)]
pub enum GbaError {
    #[error("I/O error")]
    IoError(#[from] std::io::Error),
    #[error("FPGA error")]
    FpgaError(#[from] crate::device::drivers::fpga::Error),
}

#[allow(unused)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
enum SaveType {
    /// No backup
    #[default]
    None,
    /// EEPROM - Autodetect Size
    EepromAuto,
    /// EEPROM, 512B
    Eeprom512,
    /// EEPROM, 8KiB
    Eeprom8K,
    /// SRAM or FRAM, 32 KiB
    Sram,
    /// Flash 64KiB
    Flash64K,
    /// Flash 128KiB
    Flash128K,
}

impl SaveType {
    fn get_size(self) -> usize {
        match self {
            Self::None => 0,
            Self::EepromAuto | Self::Eeprom8K => 8 * 1024,
            Self::Eeprom512 => 512,
            Self::Sram => 32 * 1024,
            Self::Flash64K => 64 * 1024,
            Self::Flash128K => 128 * 1024,
        }
    }
}

#[derive(Copy, Clone)]
struct EmulatedCartridgeConfig {
    pub save_type: SaveType,
    pub has_rumble: bool,
    pub has_rtc: bool,
    pub has_accel: bool,
    pub has_gyro: bool,
    pub has_solar: bool,
}

impl EmulatedCartridgeConfig {
    pub const DISABLED: u32 = 0;

    const fn from_save_type(save_type: SaveType) -> Self {
        EmulatedCartridgeConfig {
            save_type,
            has_rumble: false,
            has_rtc: false,
            has_accel: false,
            has_gyro: false,
            has_solar: false,
        }
    }

    fn as_config_u32(self) -> u32 {
        let backup: u32 = match self.save_type {
            SaveType::None => 0b0000,
            SaveType::Sram => 0b0001,
            SaveType::Flash64K => 0b0010,
            SaveType::Flash128K => 0b0110,
            SaveType::EepromAuto => 0b1011,
            SaveType::Eeprom512 => 0b0011,
            SaveType::Eeprom8K => 0b0111,
        };
        let has_gpio = self.has_rumble || self.has_rtc || self.has_gyro;
        1 | (backup << 1)
            | ((has_gpio as u32) << 5)
            | ((self.has_rumble as u32) << 6)
            | ((self.has_rtc as u32) << 7)
            | ((self.has_accel as u32) << 8)
            | ((self.has_gyro as u32) << 9)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RomHeader {
    game_title: [u8; 12],
    game_code: [u8; 4],
}

impl RomHeader {
    fn parse(header: [u8; ROM_HEADER_LENGTH]) -> RomHeader {
        RomHeader {
            game_title: header[0xA0..0xAC].try_into().unwrap(),
            game_code: header[0xAC..0xB0].try_into().unwrap(),
        }
    }
}

impl Display for RomHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let title = String::from_utf8_lossy(&self.game_title);
        let code = String::from_utf8_lossy(&self.game_code);
        write!(f, "\'{}\' ({})", title, code)
    }
}

/// Driver for GBA FPGA module
pub struct Gba {
    /// Emulated cartridge config.
    emu_cart_config: Option<EmulatedCartridgeConfig>,

    rom_header: Option<RomHeader>,
    rom_file_size: u32,
    save_type_detector: SaveTypeDetector,
    rtc_state: Option<RtcState>,
}

impl Gba {
    pub fn new() -> Self {
        Gba {
            emu_cart_config: None,

            rom_header: None,
            rom_file_size: 0,
            save_type_detector: SaveTypeDetector::new(),
            rtc_state: None,
        }
    }

    fn get_bios_path() -> &'static str {
        if kvs::keys::GBA_SKIP_BOOT_ANIM.get().unwrap() {
            "gba.bios-fast.bin"
        } else {
            "gba.bios.bin"
        }
    }

    /// Prepare to load a new cartridge (physical or emulated)
    fn initialize(&mut self, device: &mut Device) -> Result<(), GbaError> {
        device.imu.disable_gyro().unwrap();
        device.imu.disable_accel().unwrap();

        // Other config
        device.fpga.write_u32(
            REG_GB_PLAYER,
            kvs::keys::GBA_ENABLE_GBP.get().unwrap() as u32,
        )?;

        // Disable Vblank IRQ
        device.fpga.disable_interrupt(fpga::Irq::ModuleVblank)?;

        // Color correction
        let correction: &ColorCorrection = {
            use color_correction::presets::*;
            let corrections = [&IDENTITY, &GBC_GBA, &GBA_AGS101, &NDS, &NDS_LITE, &NSO_GBA];
            let setting = kvs::keys::GBA_COLOR_PROFILE.get().unwrap() as usize;
            corrections.get(setting).unwrap_or(&&IDENTITY)
        };
        correction.configure(device, COLOR_CORRECTION_BASE)?;

        Ok(())
    }

    pub fn get_core_info() -> CoreInfo {
        CoreInfo {
            id: "Game-Bub.GBA".try_into().unwrap(),
            name: "Game Boy Advance".try_into().unwrap(),
            author: "Game Bub".try_into().unwrap(),
            is_built_in: true,
            core_dir: PathBuf::new(),
            files: [
                CoreFile {
                    id: 0,
                    label: "ROM".try_into().unwrap(),
                    extensions: [".gba".try_into().unwrap()].into_iter().collect(),
                    filename: None,

                    optional: true,
                    read_only: true,
                    user_selected: true,
                    dependent_on_0: false,
                    initialize: false,

                    address: 0x3000_0000, // SDRAM
                    max_size: 32 * 1024 * 1024,
                    exact_size: 0,
                    max_transfer_speed: 20_000, // 20 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits32,
                },
                CoreFile {
                    id: 1,
                    label: "Save".try_into().unwrap(),
                    extensions: [".sav".try_into().unwrap()].into_iter().collect(),
                    filename: None,

                    optional: true,
                    read_only: false,
                    user_selected: false,
                    dependent_on_0: true,
                    initialize: true,

                    address: 0x4000_0000, // SRAM
                    max_size: 128 * 1024 + 16,
                    exact_size: 0,
                    max_transfer_speed: 10_000, // 10 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits16,
                },
                CoreFile {
                    id: 2,
                    label: "BIOS".try_into().unwrap(),
                    extensions: [".bin".try_into().unwrap()].into_iter().collect(),
                    filename: None, // TODO

                    optional: false,
                    read_only: true,
                    user_selected: false,
                    dependent_on_0: false,
                    initialize: false,

                    address: 0x1000_0000,
                    max_size: 0,
                    exact_size: 16 * 1024,
                    max_transfer_speed: 20_000, // 20 MB/s
                    transfer_word_size: fpga::FpgaSpiWordSize::Bits32,
                },
            ]
            .into_iter()
            .collect(),
            settings: [
                CoreSetting {
                    id: SETTING_RESET,
                    label: "Reset Core".into(),
                    address: 0x0000_2000,
                    mask: 0,
                    default: 0,
                    inner: CoreSettingType::Action { value: 1 },
                },
                CoreSetting {
                    id: SETTING_COLOR_CORRECTIONS,
                    label: "Color Corrections".into(),
                    address: 0xFFFF_FFFF,
                    mask: 0,
                    default: 1,
                    inner: CoreSettingType::List {
                        items: ["None", "GBA", "GBA SP", "NDS", "NDS Lite", "NSO GBA"]
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
                    id: SETTING_GAME_BOY_PLAYER,
                    label: "Enable Game Boy Player".into(),
                    address: 0xFFFF_FFFF,
                    mask: 0,
                    default: 1,
                    inner: CoreSettingType::Checkbox { value: 1 },
                },
            ]
            .into_iter()
            .collect(),
            bitstream: crate::util::get_system_file_path("gba.bit.hs"),
        }
    }
}

impl Bitstream for Gba {
    fn on_vblank_irq(&mut self) {
        let mut device = Device::lock();

        // Read IMU
        let has_gyro = self.emu_cart_config.as_ref().map_or(false, |h| h.has_gyro);
        if has_gyro {
            let gyro_sample = device.imu.read_gyro().unwrap();
            let gyro_z = ((0x700 as f32) - gyro_sample.z) as u16;
            device
                .fpga
                .write_u32(REG_IMU_GYRO_Z, gyro_z as u32)
                .unwrap();
        }
        let has_accel = self.emu_cart_config.as_ref().map_or(false, |h| h.has_accel);
        if has_accel {
            let accel_sample = device.imu.read_accel().unwrap();
            let accel_x = ((0x3A0 as f32) + ((0x1D0 as f32) * -accel_sample.x)) as u16;
            let accel_y = ((0x3A0 as f32) + ((0x1D0 as f32) * -accel_sample.y)) as u16;
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
}

impl CoreHandler for Gba {
    fn as_legacy_bitstream(&mut self) -> &mut dyn super::Bitstream {
        self
    }

    fn on_after_program(&mut self) {
        Device::lock().fpga.set_system_clock_rate(SYSTEM_CLOCK_RATE);
    }

    fn get_file_path_override(&mut self, id: u16) -> Option<PathBuf> {
        if id == FILE_BIOS {
            Some(crate::util::get_system_file_path(Self::get_bios_path()))
        } else {
            None
        }
    }

    fn on_before_file_load(&mut self, id: u16, file: &mut File) -> Result<(), String> {
        if id == FILE_ROM {
            self.rom_file_size = file.metadata().map_err(|_| "I/O")?.len() as u32;
            let mut rom_header = [0u8; ROM_HEADER_LENGTH];
            file.read(&mut rom_header).map_err(|_| "I/O")?;
            file.seek(std::io::SeekFrom::Start(0)).map_err(|_| "I/O")?;
            let rom_header = RomHeader::parse(rom_header);
            self.emu_cart_config = game_db::lookup(&rom_header.game_code);
            self.rom_header = Some(rom_header);
        } else if id == FILE_SAVE {
            let emu_cart_config = self.emu_cart_config.as_ref().unwrap();
            if emu_cart_config.has_rtc {
                let save_size = emu_cart_config.save_type.get_size();
                file.seek(std::io::SeekFrom::Start(save_size as u64))
                    .map_err(|_| "I/O")?;
                let mut buf = [0u8; 16];
                let n = file.read(&mut buf).map_err(|_| "I/O")?;
                if n == 16 {
                    let prev_state = RtcState::from_disk(buf[0..8].try_into().unwrap());
                    let rtc_timestamp = u64::from_le_bytes(buf[8..16].try_into().unwrap());
                    let elapsed = Device::lock()
                        .get_datetime()
                        .unix_timestamp()
                        .saturating_sub_unsigned(rtc_timestamp);

                    match prev_state.to_offset_date_time() {
                        Ok(time) => {
                            let datetime = time.saturating_add(time::Duration::seconds(elapsed));
                            let new_state = RtcState::from_offset_date_time(datetime);
                            log::info!(
                                "Loaded saved RTC state: {:?}, elapsed={}",
                                new_state,
                                elapsed
                            );
                            self.rtc_state = Some(new_state);
                        }
                        Err(_) => {
                            log::warn!("Saved RTC state invalid");
                        }
                    }
                }
                file.seek(std::io::SeekFrom::Start(0)).map_err(|_| "I/O")?;
            }
        }
        Ok(())
    }

    fn on_during_file_load(&mut self, id: u16, data: &[u8]) {
        if id != FILE_ROM {
            return;
        }
        if self.emu_cart_config.is_none() {
            self.save_type_detector.process(data);
        }
    }

    fn on_after_file_load(&mut self, id: u16) {
        if id != FILE_ROM {
            return;
        }
        match self.emu_cart_config {
            Some(config) => log::info!("Using save config: {:?}", config.save_type),
            None => {
                log::info!("Detected save type: {:?}", self.save_type_detector.get());
                self.emu_cart_config = Some(EmulatedCartridgeConfig::from_save_type(
                    self.save_type_detector.get(),
                ));
            }
        }
    }

    fn on_before_run(&mut self) -> Result<(), String> {
        let mut device = Device::lock();
        self.initialize(&mut device).map_err(|e| e.to_string())?;

        if let Some(_rom_header) = self.rom_header.as_ref() {
            let emu_cart_config = self.emu_cart_config.unwrap();

            // Configure emulated cartridge control registers
            let _ = device
                .fpga
                .write_u32(REG_EMU_CART_CONFIG, emu_cart_config.as_config_u32());
            let _ = device
                .fpga
                .write_u32(REG_EMU_CART_ROM_SIZE, self.rom_file_size - 1);

            // Update RTC state
            let rtc_state = match self.rtc_state {
                Some(state) => state,
                None => {
                    let datetime = device.get_datetime();
                    RtcState::from_offset_date_time(datetime)
                }
            };
            let (rtc_lo, rtc_hi) = rtc_state.to_fpga();
            let _ = device.fpga.write_u32(REG_RTC_LO, rtc_lo);
            let _ = device.fpga.write_u32(REG_RTC_HI, rtc_hi);

            // If IMU is needed, enable vsync IRQ
            let mut need_vblank = false;
            if emu_cart_config.has_gyro {
                device.imu.enable_gyro().unwrap();
                need_vblank = true;
            }
            if emu_cart_config.has_accel {
                device.imu.enable_accel().unwrap();
                need_vblank = true;
            }
            if need_vblank {
                device
                    .fpga
                    .enable_interrupt(fpga::Irq::ModuleVblank)
                    .unwrap();
            }
        } else {
            // Switch to physical cartridge.
            let _ = device
                .fpga
                .write_u32(REG_EMU_CART_CONFIG, EmulatedCartridgeConfig::DISABLED);
        }

        // Warn if using built-in GBA bios
        if kvs::keys::GBA_BIOS_WARNING.get().unwrap_or(true) {
            if !Path::new("/sdcard/system/gba.bios.bin").is_file() {
                ui::send(ui::Message::Notification(ui::Notification::new_long(
                    "No GBA BIOS provided:\nsome games may have bugs\n(see User Guide)".into(),
                )));
            }
        }

        Ok(())
    }

    fn get_file_size(&mut self, id: u16) -> Option<u32> {
        assert!(id == FILE_SAVE);
        let size = self
            .emu_cart_config
            .as_ref()
            .map_or(0, |e| e.save_type.get_size()) as u32;
        Some(size)
    }

    fn on_after_file_save(&mut self, id: u16, file: &mut File) -> Result<(), String> {
        assert!(id == FILE_SAVE);

        // Save RTC
        if self.emu_cart_config.as_ref().map_or(false, |e| e.has_rtc) {
            let mut device = Device::lock();
            let rtc_lo = device.fpga.read_u32(REG_RTC_LO).unwrap();
            let rtc_hi = device.fpga.read_u32(REG_RTC_HI).unwrap();
            let rtc_state = RtcState::from_fpga(rtc_lo, rtc_hi);
            let timestamp = device.get_datetime().unix_timestamp();

            file.write(&rtc_state.to_disk()).map_err(|_| "I/O")?;
            file.write(&(timestamp as u64).to_le_bytes())
                .map_err(|_| "I/O")?;
            log::info!("Wrote RTC state: {:?}", rtc_state);
        }
        Ok(())
    }

    fn on_focus_changed(&mut self, has_focus: bool) {
        let paused = !has_focus;
        let mut device = Device::lock();

        // Enable/disable IMU as needed
        if self.emu_cart_config.as_ref().map_or(false, |h| h.has_gyro) {
            if paused {
                device.imu.disable_gyro().unwrap();
            } else {
                device.imu.enable_gyro().unwrap();
            }
        }
        if self.emu_cart_config.as_ref().map_or(false, |h| h.has_accel) {
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
            (
                SETTING_COLOR_CORRECTIONS,
                kvs::keys::GBA_COLOR_PROFILE.get().unwrap() as u32,
            ),
            (
                SETTING_GAME_BOY_PLAYER,
                kvs::keys::GBA_ENABLE_GBP.get().unwrap() as u32,
            ),
        ]
    }

    fn on_setting_changed(&mut self, id: u16, value: u32) {
        let mut device = Device::lock();
        match id {
            SETTING_COLOR_CORRECTIONS => {
                kvs::keys::GBA_COLOR_PROFILE.set(&(value as i32));
                let correction: &ColorCorrection = {
                    use color_correction::presets::*;
                    let corrections = [&IDENTITY, &GBC_GBA, &GBA_AGS101, &NDS, &NDS_LITE, &NSO_GBA];
                    corrections.get(value as usize).unwrap_or(&&IDENTITY)
                };
                let _ = correction.configure(&mut device, COLOR_CORRECTION_BASE);
            }
            SETTING_GAME_BOY_PLAYER => {
                kvs::keys::GBA_ENABLE_GBP.set(&(value == 1));
                let _ = device.fpga.write_u32(REG_GB_PLAYER, value);
            }
            _ => {}
        }
    }
}
