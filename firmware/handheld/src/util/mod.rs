pub mod background_io;

use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub fn open_system_file(relative_path: &str) -> std::io::Result<File> {
    File::open(get_system_file_path(relative_path))
}

pub fn get_system_file_path(relative_path: &str) -> PathBuf {
    let path = Path::new("/sdcard/system/").join(relative_path);
    if path.is_file() {
        return path;
    }
    Path::new("/system/").join(relative_path)
}

#[allow(unused)]
pub fn copy_file(from: &Path, to: &Path) -> std::io::Result<u64> {
    let mut reader = std::fs::File::open(from)?;
    // Workaround for an issue with esp-idf
    let metadata = reader.metadata()?.file_type();

    if !metadata.is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }

    let mut writer = std::fs::File::create(to)?;
    Ok(std::io::copy(&mut reader, &mut writer)?)
}

/// Whether a file name ends with one of the extensions (including the dot), ignoring case.
pub fn has_extension(name: &str, extensions: &[&str]) -> bool {
    let name = name.as_bytes();
    extensions.iter().any(|ext| {
        let ext = ext.as_bytes();
        name.len() >= ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
    })
}
