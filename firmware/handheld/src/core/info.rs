use arrayvec::{ArrayString, ArrayVec};
use serde::Deserialize;
use slint::SharedString;
use std::{
    fs::File,
    io::{BufReader, ErrorKind},
    path::{Path, PathBuf},
};

use crate::device::drivers::fpga;

pub const DIR_CORES: &str = "/sdcard/cores/";

const MAX_CORE_BITSTREAMS: usize = 16;
const MAX_CORE_FILES: usize = 8;
const MAX_CORE_SETTINGS: usize = 16;
const FRAMEWORK_ADDR_START: u32 = 0xF000_0000;
const MIN_TRANSFER_SPEED_KBS: u32 = 1_000;
const MAX_TRANSFER_SPEED_KBS: u32 = 1_000_000;

#[derive(Deserialize)]
pub struct CoreListEntry {
    pub id: ArrayString<32>,
    pub name: ArrayString<32>,
    pub author: ArrayString<32>,
}

pub struct CoreInfo {
    pub id: ArrayString<32>,
    #[expect(unused)]
    pub name: ArrayString<32>,
    #[expect(unused)]
    pub author: ArrayString<32>,
    pub is_built_in: bool,
    pub core_dir: PathBuf,
    pub files: Vec<CoreFile>,
    pub settings: Vec<CoreSetting>,
    pub bitstream: PathBuf,
    pub uses_cartridge: CoreCartridgeMode,
}

#[derive(Default, Copy, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreCartridgeMode {
    #[default]
    No,
    Yes,
    IfSelected,
}

#[derive(Deserialize)]
pub struct CoreFile {
    pub id: u16,
    pub label: ArrayString<16>,

    /// If set, the file will be loaded from this path relative to the asset path.
    #[serde(default)]
    pub filename: Option<ArrayString<32>>,

    /// List of file extensions (optional).
    #[serde(default)]
    pub extensions: ArrayVec<ArrayString<8>, 4>,

    /// If true, the core will still run if the file is not loaded.
    #[serde(default = "default_true")]
    pub optional: bool,
    /// If true, file is treated as read-only, won't be saved at core end.
    #[serde(default)]
    pub read_only: bool,
    /// If true, the file path is selected by the user (filtered by extensions).
    #[serde(default)]
    pub user_selected: bool,
    /// If true, dependent on the file with ID 0 (and the path is determined based on that path + this extension).
    #[serde(default)]
    pub dependent_on_0: bool,
    /// If true, if the file is not loaded, the region will still be initialized with 0xFFs.
    #[serde(default)]
    pub initialize: bool,
    /// If true, the file is not automatically loaded at startup.
    #[serde(default)]
    pub on_demand: bool,

    /// The address to load the file to.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub address: u32,
    /// Maximum size of the file.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub max_size: u32,
    /// Exact size of the file.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_hex_u32")]
    pub exact_size: u32,
    /// Maximum read/write speed when loading/saving the file (in KB/s). Default 5000 KB/sec
    #[serde(default = "default_transfer_speed")]
    pub max_transfer_speed: u32,

    /// Word size during transfer
    /// TODO: remove this, make all transfers 32-bit
    #[serde(skip)]
    #[serde(default = "default_transfer_word_size")]
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
    #[cfg(not(feature = "no_beta_cores"))]
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
            if !entry
                .metadata()
                .map(|t| t.file_type().is_dir())
                .unwrap_or(false)
            {
                continue;
            }

            let file_name = entry.file_name();
            let Some(folder_name) = file_name.to_str() else {
                continue;
            };
            if folder_name == "Game-Bub.GB" || folder_name == "Game-Bub.GBA" {
                continue;
            }

            /// Helper struct to extract only high-level info from core
            #[derive(Deserialize)]
            struct MinimalCoreInfo {
                metadata: CoreListEntry,
            }

            let info = match try_read_json::<MinimalCoreInfo>(&entry.path(), "core.json") {
                Ok(Some(info)) => info.metadata,
                Ok(None) => continue,
                Err(e) => {
                    log::warn!("{folder_name}: {e}");
                    continue;
                }
            };
            if info.id.as_str() != folder_name {
                log::warn!(
                    "Core ID '{}' does not match folder '{folder_name}'",
                    info.id
                );
                continue;
            }
            cores.push(info);
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

    #[derive(Deserialize, Default)]
    struct JsonCoreHardware {
        #[serde(default)]
        pub cartridge_enable: CoreCartridgeMode,
        #[serde(default)]
        #[expect(unused)]
        pub cartridge_selectable: bool,
    }

    #[derive(Deserialize)]
    struct JsonCoreInfo {
        metadata: JsonCoreMetadata,
        #[serde(deserialize_with = "deserialize_bounded_vec::<_, _, MAX_CORE_BITSTREAMS>")]
        bitstreams: Vec<JsonCoreBitstream>,
        #[serde(default)]
        hardware: JsonCoreHardware,
    }

    #[derive(Deserialize)]
    struct JsonSettings {
        #[serde(deserialize_with = "deserialize_bounded_vec::<_, _, MAX_CORE_SETTINGS>")]
        settings: Vec<CoreSetting>,
    }

    #[derive(Deserialize)]
    struct JsonFiles {
        #[serde(deserialize_with = "deserialize_bounded_vec::<_, _, MAX_CORE_FILES>")]
        files: Vec<CoreFile>,
    }

    if id.is_empty() || id == "." || id == ".." || id.contains('/') {
        return Err(format!("Invalid core ID '{id}'"));
    }

    let mut core_dir = PathBuf::from(DIR_CORES);
    core_dir.push(id);

    let json_core: JsonCoreInfo = try_read_json(&core_dir, "core.json")?
        .ok_or_else(|| format!("Missing core.json in /cores/{id}"))?;

    if json_core.metadata.id.as_str() != id {
        return Err(format!(
            "core.json: metadata.id '{}' does not match dir '{id}'",
            json_core.metadata.id
        ));
    }
    if json_core.metadata.name.trim().is_empty() || json_core.metadata.author.trim().is_empty() {
        return Err("core.json: metadata name and author cannot be empty".into());
    }

    // Find the right bitstream (TODO: use a visitor that extracts the right one).
    // https://serde.rs/stream-array.html
    let target = get_device_target();
    let bitstream = json_core
        .bitstreams
        .iter()
        .find_map(|b| (b.target.as_str() == target).then_some(b.filename))
        .ok_or_else(|| format!("core.json: no bitstream for target '{target}'"))?;
    if !bitstream.ends_with(".bit") || !validate_subpath(bitstream.as_str()) {
        return Err(format!(
            "core.json: invalid bitstream filename '{bitstream}'"
        ));
    }
    let bitstream_path = core_dir.join(bitstream.as_str());
    if !bitstream_path.is_file() {
        return Err(format!("core.json: bitstream '{bitstream}' not found"));
    }

    let settings = try_read_json::<JsonSettings>(&core_dir, "settings.json")?
        .map(|s| s.settings)
        .unwrap_or_default();

    let files = try_read_json::<JsonFiles>(&core_dir, "files.json")?
        .map(|f| f.files)
        .unwrap_or_default();
    validate_files(&files)?;

    Ok(CoreInfo {
        id: json_core.metadata.id,
        name: json_core.metadata.name,
        author: json_core.metadata.author,
        uses_cartridge: json_core.hardware.cartridge_enable,
        is_built_in: false,
        files,
        bitstream: bitstream_path,
        settings,
        core_dir,
    })
}

fn validate_subpath(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}

fn validate_files(files: &[CoreFile]) -> Result<(), String> {
    let file_0 = files.iter().find(|f| f.id == 0);
    for (i, f) in files.iter().enumerate() {
        let id = f.id;
        if files[..i].iter().any(|prev| prev.id == id) {
            return Err(format!("files.json: duplicate file id {id}"));
        }

        let sources =
            (f.filename.is_some() as u32) + (f.user_selected as u32) + (f.dependent_on_0 as u32);
        if sources != 1 {
            return Err(format!(
                "files.json [id={id}]:\nmust set exactly one of filename, user_selected, or dependent_on_0"
            ));
        }

        if f.extensions
            .iter()
            .any(|ext| ext.is_empty() || ext.starts_with('.') || ext.contains('/'))
        {
            return Err(format!("files.json [id={id}]:\ninvalid file extension"));
        }
        if f.user_selected && f.extensions.is_empty() {
            return Err(format!(
                "files.json [id={id}]:\nuser_selected requires extensions"
            ));
        }

        if let Some(filename) = f.filename.as_ref() {
            if !validate_subpath(filename.as_str()) {
                return Err(format!(
                    "files.json [id={id}]:\ninvalid filename '{filename}'"
                ));
            }
        }

        if f.dependent_on_0 {
            let valid_f0 = id != 0
                && !f.extensions.is_empty()
                && file_0.is_some_and(|f0| f0.user_selected && (!f0.optional || f.optional));
            if !valid_f0 {
                return Err(format!(
                    "files.json [id={id}]:\ninvalid dependent_on_0 configuration"
                ));
            }
        }

        if f.max_transfer_speed < MIN_TRANSFER_SPEED_KBS
            || f.max_transfer_speed > MAX_TRANSFER_SPEED_KBS
        {
            return Err(format!(
                "files.json [id={id}]:\ninvalid max_transfer_speed {}",
                f.max_transfer_speed
            ));
        }

        if !f.on_demand {
            if f.address % 4 != 0 || f.address >= FRAMEWORK_ADDR_START {
                return Err(format!(
                    "files.json [id={id}]: invalid address 0x{:08X}",
                    f.address
                ));
            }
        }
    }
    Ok(())
}

fn try_read_json<T: serde::de::DeserializeOwned>(
    core_dir: &Path,
    filename: &str,
) -> Result<Option<T>, String> {
    let file = match File::open(core_dir.join(filename)) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Failed to open {filename}: {e}")),
    };
    let reader = BufReader::with_capacity(256, file);
    serde_json::from_reader(reader)
        .map(Some)
        .map_err(|e| format!("Invalid {filename}: {e}"))
}

fn get_device_target() -> &'static str {
    #[cfg(feature = "rev1")]
    const TARGET: &str = "gamebub_rev1";
    #[cfg(feature = "rev2")]
    const TARGET: &str = "gamebub_rev2";
    #[cfg(feature = "rev3")]
    const TARGET: &str = "gamebub_rev3";
    #[cfg(feature = "rev4")]
    const TARGET: &str = "gamebub_rev4";

    TARGET
}

/// Serde helper to deserialize a Vec with an upper bound enforced during parsing.
fn deserialize_bounded_vec<'de, D, T, const MAX: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T, const MAX: usize>(Vec<T>);

    impl<'de, T: Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de> for Visitor<T, MAX> {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a list with at most {MAX} items")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            mut self,
            mut seq: A,
        ) -> Result<Vec<T>, A::Error> {
            while let Some(elem) = seq.next_element()? {
                if self.0.len() == MAX {
                    return Err(serde::de::Error::custom(format_args!(
                        "too many items (max {MAX})"
                    )));
                }
                self.0.push(elem);
            }
            Ok(self.0)
        }
    }

    deserializer.deserialize_seq(Visitor::<T, MAX>(Vec::new()))
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

fn default_true() -> bool {
    true
}

fn default_transfer_speed() -> u32 {
    5000
}

fn default_transfer_word_size() -> fpga::FpgaSpiWordSize {
    fpga::FpgaSpiWordSize::Bits32
}
