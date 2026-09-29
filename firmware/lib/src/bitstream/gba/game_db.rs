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
static DATABASE: &[(&'static [u8; 4], EmulatedCartridgeConfig)] = &[
    // Pokemon Sapphire
    (b"AXPJ", config!(Flash128K, has_rtc)),
    (b"AXPE", config!(Flash128K, has_rtc)),
    (b"AXPP", config!(Flash128K, has_rtc)),
    (b"AXPI", config!(Flash128K, has_rtc)),
    (b"AXPS", config!(Flash128K, has_rtc)),
    (b"AXPD", config!(Flash128K, has_rtc)),
    (b"AXPF", config!(Flash128K, has_rtc)),
    // Pokemon Ruby
    (b"AXVJ", config!(Flash128K, has_rtc)),
    (b"AXVP", config!(Flash128K, has_rtc)),
    (b"AXVE", config!(Flash128K, has_rtc)),
    (b"AXVI", config!(Flash128K, has_rtc)),
    (b"AXVS", config!(Flash128K, has_rtc)),
    (b"AXVD", config!(Flash128K, has_rtc)),
    (b"AXVF", config!(Flash128K, has_rtc)),
    // Pokemon Emerald
    (b"BPEJ", config!(Flash128K, has_rtc)),
    (b"BPEE", config!(Flash128K, has_rtc)),
    (b"BPEP", config!(Flash128K, has_rtc)),
    (b"BPEI", config!(Flash128K, has_rtc)),
    (b"BPES", config!(Flash128K, has_rtc)),
    (b"BPED", config!(Flash128K, has_rtc)),
    (b"BPEF", config!(Flash128K, has_rtc)),
    // Sennen Kazoku
    (b"BKAJ", config!(Flash128K, has_rtc)),
    // Legendz - Yomigaeru Shiren no Shima
    (b"BLJJ", config!(Flash64K, has_rtc)),
    (b"BLJK", config!(Flash64K, has_rtc)),
    // Legendz - Sign of Nekuromu
    (b"BLVJ", config!(Flash64K, has_rtc)),
    // RockMan EXE 4.5 - Real Operation
    (b"BR4J", config!(Flash64K, has_rtc)),
    // Boktai: The Sun is in Your Hand
    (b"U3IJ", config!(EepromAuto, has_rtc, has_solar)),
    (b"U3IE", config!(EepromAuto, has_rtc, has_solar)),
    (b"U3IP", config!(EepromAuto, has_rtc, has_solar)),
    // Koro Koro Puzzle - Happy Panechu!
    (b"KHPJ", config!(EepromAuto, has_accel)),
    // Yoshi's Universal Gravitation
    (b"KYGJ", config!(EepromAuto, has_accel)),
    (b"KYGE", config!(EepromAuto, has_accel)),
    (b"KYGP", config!(EepromAuto, has_accel)),
    // Wario Ware Twisted
    (b"RZWJ", config!(Sram, has_rumble, has_gyro)),
    (b"RZWE", config!(Sram, has_rumble, has_gyro)),
    // Boktai 2: Solar Boy Django
    (b"U32J", config!(EepromAuto, has_rtc, has_solar)),
    (b"U32E", config!(EepromAuto, has_rtc, has_solar)),
    (b"U32P", config!(EepromAuto, has_rtc, has_solar)),
    // Shin Bokura no Taiyou: Gyakushuu no Sabata
    (b"U33J", config!(EepromAuto, has_rtc, has_solar)),
    // Drill Dozer
    (b"V49J", config!(Sram, has_rumble)),
    (b"V49E", config!(Sram, has_rumble)),
    (b"V49P", config!(Sram, has_rumble)),
    // Goodboy Galaxy
    (b"2GBP", config!(Sram, has_rumble)),
    // Apotris
    (b"2ATE", config!(Sram, has_rumble)),
];

pub fn lookup(key: &[u8; 4]) -> Option<EmulatedCartridgeConfig> {
    DATABASE
        .iter()
        .find(|(&code, _)| *key == code)
        .map(|(_, config)| config.clone())
}
