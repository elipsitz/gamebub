use std::path::PathBuf;

use super::KvsKey;

/// Setup / OOBE stage
pub static SETUP_STAGE: KvsKey<u32> = KvsKey::new_with_default("setup-stage", 0);

/// Total uptime, in seconds.
pub static UPTIME: KvsKey<u32> = KvsKey::new_with_default("uptime", 0);

/// The full path of the last selected ROM.
pub static LAST_ROM_PATH: KvsKey<PathBuf> = KvsKey::new("last-rom-path");

/// The last volume level.
pub static VOLUME: KvsKey<u8> = KvsKey::new_with_default("volume", 128);

/// The last brightness level.
pub static BRIGHTNESS: KvsKey<f32> = KvsKey::new_with_default("brightness", 0.50);

/// Whether dark mode is enabled.
pub static DARK_MODE: KvsKey<bool> = KvsKey::new_with_default("dark-mode", false);

/// Whether to use DMG mode (instead of CGB mode)
pub static GB_IS_DMG: KvsKey<bool> = KvsKey::new_with_default("gb-is-dmg", false);

/// Whether to skip DMG/CGB boot animation
pub static GB_SKIP_BOOT_ANIM: KvsKey<bool> = KvsKey::new_with_default("gb-no-anim", false);

/// DMG color palette
pub static DMG_COLOR_PALETTE: KvsKey<i32> = KvsKey::new_with_default("dmg-colors", 1);

/// CGB color profile
pub static CGB_COLOR_PROFILE: KvsKey<i32> = KvsKey::new_with_default("cgb-colors", 1);

/// Whether to skip GBA boot animation.
pub static GBA_SKIP_BOOT_ANIM: KvsKey<bool> = KvsKey::new_with_default("gba-no-anim", false);

/// GBA color profile
pub static GBA_COLOR_PROFILE: KvsKey<i32> = KvsKey::new_with_default("gba-colors", 1);

/// Whether to enable Game Boy Player functionality
pub static GBA_ENABLE_GBP: KvsKey<bool> = KvsKey::new_with_default("gba-enable-gbp", true);

// Whether to use the SGB button layout for GBA
pub static GBA_SGB_BUTTONS: KvsKey<bool> = KvsKey::new_with_default("gba-sgb-buttons", false);

// Whether to use the SGB button layout for GB/GBC
pub static GB_SGB_BUTTONS: KvsKey<bool> = KvsKey::new_with_default("gb-sgb-buttons", false);

/// Startup action.
pub static STARTUP_ACTION: KvsKey<i32> = KvsKey::new_with_default("startup-action", 0);

/// Last firmware version
pub static LAST_FIRMWARE_VERSION: KvsKey<String> = KvsKey::new("last-fw-version");

pub fn flush_all() {
    SETUP_STAGE.flush();
    UPTIME.flush();
    LAST_ROM_PATH.flush();
    VOLUME.flush();
    BRIGHTNESS.flush();
    DARK_MODE.flush();
    GB_IS_DMG.flush();
    GB_SKIP_BOOT_ANIM.flush();
    DMG_COLOR_PALETTE.flush();
    CGB_COLOR_PROFILE.flush();
    GBA_SKIP_BOOT_ANIM.flush();
    GBA_COLOR_PROFILE.flush();
    GBA_ENABLE_GBP.flush();
    GBA_SGB_BUTTONS.flush();
    GB_SGB_BUTTONS.flush();
    STARTUP_ACTION.flush();
    LAST_FIRMWARE_VERSION.flush();
}
