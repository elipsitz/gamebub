use arrayvec::{ArrayString, ArrayVec};
use serde::Deserialize;
use slint::SharedString;
use std::{fs::File, io::BufReader, path::PathBuf};

use crate::device::drivers::fpga;

pub const DIR_CORES: &str = "/sdcard/cores/";

#[derive(Deserialize)]
pub struct CoreListEntry {
    pub id: ArrayString<32>,
    pub name: ArrayString<32>,
    pub author: ArrayString<32>,
}

#[allow(unused)]
pub struct CoreInfo {
    pub id: ArrayString<32>,
    pub name: ArrayString<32>,
    pub author: ArrayString<32>,
    pub is_built_in: bool,
    pub files: ArrayVec<CoreFile, 8>,
    pub settings: Vec<CoreSetting>,
    pub bitstream: PathBuf,
}

#[allow(unused)]
pub struct CoreFile {
    pub id: u16,
    pub label: ArrayString<16>,
    /// If set, the file will be loaded from this path relative to the asset path.
    pub filename: Option<ArrayString<32>>,

    /// List of file extensions (optional).
    pub extensions: ArrayVec<ArrayString<8>, 4>,

    /// If true, the core will still run if the file is not loaded.
    pub optional: bool,
    /// If true, file will be loaded at core start and saved at core end.
    // persistent: bool,
    /// If true, file is treated as read-only, won't be saved at core end.
    pub read_only: bool,
    /// If true, the file path is selected by the user (filtered by extensions).
    pub user_selected: bool,
    /// If true, dependent on the file with ID 0 (and the path is determined based on that path + this extension).
    pub dependent_on_0: bool,
    /// If true, if the file is not loaded, the region will still be initialized with 0xFFs.
    pub initialize: bool,

    /// The address to load the file to.
    /// TODO: support hex-string
    pub address: u32,
    /// Maximum size of the file.
    /// TODO: support hex-string
    pub max_size: u32,
    /// Exact size of the file.
    /// TODO: support hex-string
    pub exact_size: u32,
    /// Maximum read/write speed when loading/saving the file (in KB/s)
    pub max_transfer_speed: u32,

    /// Word size during transfer
    /// TODO: remove this, make all transfers 32-bit
    pub transfer_word_size: fpga::FpgaSpiWordSize,
}

#[derive(Deserialize)]
pub struct CoreSetting {
    pub id: u16,
    /// User-visible label
    pub label: SharedString,
    /// Address of the setting in the core
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub address: u32,
    /// Mask used when setting the value
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub mask: u32,
    /// Default value
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub default: u32,
    /// Per-type information
    #[serde(flatten)]
    pub inner: CoreSettingType,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum CoreSettingType {
    Action {
        #[serde(deserialize_with = "deserialize_hex_u32")]
        value: u32,
    },
    Checkbox {
        #[serde(deserialize_with = "deserialize_hex_u32")]
        value: u32,
    },
    List {
        items: ArrayVec<CoreSettingListItem, 8>,
    },
}

#[derive(Deserialize)]
pub struct CoreSettingListItem {
    pub label: SharedString,
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub value: u32,
}

impl CoreInfo {
    pub fn get_settings_path(&self) -> PathBuf {
        let mut p = PathBuf::from(super::DIR_SETTINGS);
        p.push(self.id);
        p.add_extension("json");
        p
    }
}

/// Get a list of all available cores.
pub fn list_cores() -> Vec<CoreListEntry> {
    // Start with built-in cores.
    let mut cores = vec![
        CoreListEntry {
            id: "Game-Bub.GB".try_into().unwrap(),
            name: "Game Boy / Game Boy Color".try_into().unwrap(),
            author: "Game Bub".try_into().unwrap(),
        },
        CoreListEntry {
            id: "Game-Bub.GBA".try_into().unwrap(),
            name: "Game Boy Advance".try_into().unwrap(),
            author: "Game Bub".try_into().unwrap(),
        },
    ];

    // Iterate over possible core directories.
    if let Ok(entries) = std::fs::read_dir(DIR_CORES) {
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    log::warn!("Failed to list core file");
                    continue;
                }
            };

            // Skip non-directories
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }

            let mut path = entry.path();
            path.push("core.json");

            let file = match File::open(&path) {
                Ok(file) => file,
                Err(_) => {
                    log::warn!("Failed to open core file");
                    continue;
                }
            };

            /// Helper struct to extract only high-level info from core
            #[derive(Deserialize)]
            struct MinimalCoreInfo {
                metadata: CoreListEntry,
            }

            let reader = BufReader::with_capacity(256, file);
            match serde_json::from_reader::<_, MinimalCoreInfo>(reader) {
                Ok(info) => cores.push(info.metadata),
                Err(e) => {
                    let filename = path.file_name().unwrap_or_default();
                    log::warn!("Error parsing core file {filename:?}: {e:?}");
                    continue;
                }
            };
        }
    }

    cores.sort_by(|a, b| a.name.cmp(&b.name));
    cores
}

/// Get full information for a core.
/// TODO: better error type?
pub fn get_core(id: &str) -> Result<CoreInfo, String> {
    // Handle built-in cores.
    match id {
        "Game-Bub.GB" => return Ok(crate::bitstream::gameboy::Gameboy::get_core_info()),
        "Game-Bub.GBA" => return Ok(crate::bitstream::gba::Gba::get_core_info()),
        _ => {}
    }

    #[derive(Deserialize)]
    struct JsonCoreMetadata {
        pub id: ArrayString<32>,
        pub name: ArrayString<32>,
        pub author: ArrayString<32>,
    }

    #[derive(Deserialize)]
    struct JsonCoreBitstream {
        pub target: ArrayString<16>,
        pub filename: ArrayString<32>,
    }

    #[derive(Deserialize)]
    struct JsonCoreInfo {
        metadata: JsonCoreMetadata,
        bitstreams: Vec<JsonCoreBitstream>,
    }

    #[derive(Deserialize)]
    struct JsonSettings {
        settings: Vec<CoreSetting>,
    }

    let mut core_dir = PathBuf::from(DIR_CORES);
    core_dir.push(id);

    // Read core.json
    let json_core: JsonCoreInfo = {
        let file =
            File::open(&core_dir.join("core.json")).map_err(|_| "Failed to open core.json")?;
        let reader = BufReader::with_capacity(256, file);
        serde_json::from_reader(reader).map_err(|e| format!("Failed to parse core.json: {e}"))?
    };

    // Find the right bitstream (TODO: use a visitor that extracts the right one).
    // https://serde.rs/stream-array.html
    let bitstream = json_core
        .bitstreams
        .iter()
        .find_map(|b| {
            if b.target.as_str() == get_device_target() && b.filename.ends_with(".bit") {
                Some(b.filename)
            } else {
                None
            }
        })
        .ok_or("No compatible bitstream")?;

    // Read settings.json
    let settings = {
        if let Ok(file) = File::open(&core_dir.join("settings.json")) {
            let reader = BufReader::with_capacity(256, file);
            let settings: JsonSettings = serde_json::from_reader(reader)
                .map_err(|e| format!("Failed to parse settings.json: {e}"))?;
            settings.settings
        } else {
            Vec::new()
        }
    };

    Ok(CoreInfo {
        id: json_core.metadata.id,
        name: json_core.metadata.name,
        author: json_core.metadata.author,
        is_built_in: false,
        files: ArrayVec::new(), // TODO
        bitstream: core_dir.join(bitstream),
        settings,
    })
}

fn get_device_target() -> &'static str {
    #[cfg(feature = "rev1")]
    const TARGET: &'static str = "gamebub_rev1";
    #[cfg(feature = "rev2")]
    const TARGET: &'static str = "gamebub_rev2";
    #[cfg(feature = "rev3")]
    const TARGET: &'static str = "gamebub_rev3";
    #[cfg(feature = "rev4")]
    const TARGET: &'static str = "gamebub_rev4";

    TARGET
}

/// Serde helper to deserialize a u32, either from a hex string or a number
fn deserialize_hex_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrStr {
        Int(u32),
        Str(ArrayString<10>),
    }
    match IntOrStr::deserialize(deserializer)? {
        IntOrStr::Int(num) => Ok(num),
        IntOrStr::Str(str) => str
            .strip_prefix("0x")
            .and_then(|s| u32::from_str_radix(s, 16).ok())
            .ok_or_else(|| serde::de::Error::custom("expected int or hex string")),
    }
}
