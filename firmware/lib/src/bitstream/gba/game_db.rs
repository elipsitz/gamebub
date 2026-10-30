use super::{EmulatedCartridgeConfig, SaveType};

macro_rules! config {
    ($save_type:ident $(, $field:ident)*) => {
        {
            #[allow(unused_mut)]
            let mut config = EmulatedCartridgeConfig::from_save_type(SaveType::$save_type);
            $(
                config.$field = true;
            )*
            config
        }
    }
}

/// The game database.
static DATABASE: &[(&'static [u8], EmulatedCartridgeConfig)] = &[
    (b"AXP", config!(Flash128K, has_rtc)), // Pokemon Sapphire
    (b"AXV", config!(Flash128K, has_rtc)), // Pokemon Ruby
    (b"BPE", config!(Flash128K, has_rtc)), // Pokemon Emerald
    (b"BKA", config!(Flash128K, has_rtc)), // Sennen Kazoku
    (b"BLJ", config!(Flash64K, has_rtc)),  // Legendz - Yomigaeru Shiren no Shima
    (b"BLV", config!(Flash64K, has_rtc)),  // Legendz - Sign of Nekuromu
    (b"BR4", config!(Flash64K, has_rtc)),  // RockMan EXE 4.5 - Real Operation
    (b"U3I", config!(EepromAuto, has_rtc, has_solar)), // Boktai: The Sun is in Your Hand
    (b"KHP", config!(EepromAuto, has_accel)), // Koro Koro Puzzle - Happy Panechu!
    (b"KYG", config!(EepromAuto, has_accel)), // Yoshi's Universal Gravitation
    (b"RZW", config!(Sram, has_rumble, has_gyro)), // Wario Ware Twisted
    (b"U32", config!(EepromAuto, has_rtc, has_solar)), // Boktai 2: Solar Boy Django
    (b"U33", config!(EepromAuto, has_rtc, has_solar)), // Shin Bokura no Taiyou: Gyakushuu no Sabata
    (b"V49", config!(Sram, has_rumble)),   // Drill Dozer
    (b"2GB", config!(Sram, has_rumble)),   // Goodboy Galaxy
    (b"2AT", config!(Sram, has_rumble)),   // Apotris
    // EverDrive auto-detection prefixes
    (b"1", config!(EepromAuto)),
    (b"2", config!(Sram)),
    (b"3", config!(Flash64K)),
    (b"4", config!(Flash128K)),
];

pub fn lookup(key: &[u8; 4]) -> Option<EmulatedCartridgeConfig> {
    DATABASE
        .iter()
        .find(|(code, _)| key.starts_with(code))
        .map(|(_, config)| config.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pokemon_fire_red() {
        assert!(lookup("BPRE".as_bytes().try_into().unwrap()).is_none());
    }

    #[test]
    fn test_pokemon_sapphire() {
        let axpj = lookup("AXPJ".as_bytes().try_into().unwrap()).unwrap();
        assert_eq!(axpj.save_type, SaveType::Flash128K);
        assert!(!axpj.has_rumble);
        assert!(axpj.has_rtc);
        assert!(!axpj.has_accel);
        assert!(!axpj.has_gyro);
        assert!(!axpj.has_solar);
    }

    #[test]
    fn test_goodboy_galaxy() {
        let twogbp = lookup("2GBP".as_bytes().try_into().unwrap()).unwrap();
        assert_eq!(twogbp.save_type, SaveType::Sram);
        assert!(twogbp.has_rumble);
        assert!(!twogbp.has_rtc);
        assert!(!twogbp.has_accel);
        assert!(!twogbp.has_gyro);
        assert!(!twogbp.has_solar);
    }

    #[test]
    fn test_unknown_flash_128k() {
        let unknown = lookup("4444".as_bytes().try_into().unwrap()).unwrap();
        assert_eq!(unknown.save_type, SaveType::Flash128K);
        assert!(!unknown.has_rumble);
        assert!(!unknown.has_rtc);
        assert!(!unknown.has_accel);
        assert!(!unknown.has_gyro);
        assert!(!unknown.has_solar);
    }
}
