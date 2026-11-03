pub mod game_db;

use serde::Deserialize;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveType {
    /// Autodetect
    #[default]
    Autodetect,
    /// No backup
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
    pub fn get_size(self) -> usize {
        match self {
            Self::None | Self::Autodetect => 0,
            Self::EepromAuto | Self::Eeprom8K => 8 * 1024,
            Self::Eeprom512 => 512,
            Self::Sram => 32 * 1024,
            Self::Flash64K => 64 * 1024,
            Self::Flash128K => 128 * 1024,
        }
    }

    pub fn autodetect(self) -> bool {
        self == Self::Autodetect
    }
}

#[derive(Copy, Clone, Deserialize, Default)]
#[serde(default)]
pub struct EmulatedCartridgeConfig {
    pub save_type: SaveType,
    pub has_rumble: bool,
    pub has_rtc: bool,
    pub has_accel: bool,
    pub has_gyro: bool,
    pub has_solar: bool,
}

impl EmulatedCartridgeConfig {
    pub const DISABLED: u32 = 0;

    pub const fn from_save_type(save_type: SaveType) -> Self {
        EmulatedCartridgeConfig {
            save_type,
            has_rumble: false,
            has_rtc: false,
            has_accel: false,
            has_gyro: false,
            has_solar: false,
        }
    }

    pub fn as_config_u32(self) -> u32 {
        let backup: u32 = match self.save_type {
            SaveType::None | SaveType::Autodetect => 0b0000,
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
