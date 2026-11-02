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

/// Compare two strings, treating runs of ASCII digits as numbers.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let a_len = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let b_len = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (a_num, b_num) = (&a[..a_len], &b[..b_len]);
                // Compare numerically: ignore leading zeros, then longer is bigger.
                let a_trim = trim_zeros(a_num);
                let b_trim = trim_zeros(b_num);
                let ordering = a_trim
                    .len()
                    .cmp(&b_trim.len())
                    .then_with(|| a_trim.cmp(b_trim))
                    .then_with(|| a_len.cmp(&b_len));
                if ordering != Ordering::Equal {
                    return ordering;
                }
                a = &a[a_len..];
                b = &b[b_len..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|&&c| c == b'0').count();
    &digits[zeros..]
}
