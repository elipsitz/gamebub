use std::{cmp::Ordering, fmt::Display};

use arrayvec::ArrayString;
use serde::Deserialize;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub pre: u8,
}

impl FirmwareVersion {
    pub fn parse(s: &str) -> Option<Self> {
        fn parse_u8(s: &str) -> Option<u8> {
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        }

        let (ver, pre_str) = match s.split_once('-') {
            Some((v, p)) => (v, Some(p)),
            None => (s, None),
        };

        let mut parts = ver.split('.');
        let major = parse_u8(parts.next()?)?;
        let minor = parse_u8(parts.next()?)?;
        let patch = match parts.next() {
            Some(p) => parse_u8(p)?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }

        let pre = match pre_str {
            None => 0,
            Some(p) => {
                let rest = p.trim_start_matches(|c: char| c.is_ascii_lowercase());
                if rest.len() == p.len() {
                    return None;
                }
                let num = parse_u8(rest.strip_prefix('.').unwrap_or(rest))?;
                if num == 0 {
                    return None;
                }
                num
            }
        };

        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl Ord for FirmwareVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        // pre == 0 denotes a full release, which sorts after any prerelease (pre > 0).
        let key = |v: &Self| (v.major, v.minor, v.patch, v.pre == 0, v.pre);
        key(self).cmp(&key(other))
    }
}

impl PartialOrd for FirmwareVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'de> Deserialize<'de> for FirmwareVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = ArrayString::<32>::deserialize(deserializer)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("invalid firmware version"))
    }
}

impl From<u32> for FirmwareVersion {
    fn from(value: u32) -> Self {
        Self {
            major: ((value >> 24) & 0xFF) as u8,
            minor: ((value >> 16) & 0xFF) as u8,
            patch: ((value >> 8) & 0xFF) as u8,
            pre: ((value >> 0) & 0xFF) as u8,
        }
    }
}

impl Display for FirmwareVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.pre != 0 {
            write!(f, "-beta.{}", self.pre)?;
        }
        Ok(())
    }
}

#[repr(C)]
struct FirmwareMetadata {
    git_commit: [u8; 20],
    version: FirmwareVersion,
}

#[used]
#[link_section = ".rodata_custom_desc"]
static FIRMWARE_METADATA: FirmwareMetadata = FirmwareMetadata {
    git_commit: GIT_COMMIT_BYTES,
    version: FIRMWARE_VERSION,
};

pub const GIT_COMMIT_BYTES: [u8; 20] = {
    const fn parse(hex: u8) -> u8 {
        if hex >= b'0' && hex <= b'9' {
            hex - b'0'
        } else if hex >= b'a' && hex <= b'f' {
            hex - b'a' + 10
        } else {
            panic!();
        }
    }

    let mut hash = [0u8; 20];
    let string = env!("GIT_COMMIT").as_bytes();
    let mut i = 0;
    while i < 20 {
        hash[i] = parse(string[i * 2]) << 4 | parse(string[i * 2 + 1]);
        i += 1;
    }
    hash
};

pub const FIRMWARE_VERSION: FirmwareVersion = {
    const fn parse_u8(bytes: &[u8], start: usize) -> u8 {
        assert!(start < bytes.len());
        let mut val: u16 = 0;
        let mut i = start;
        while i < bytes.len() {
            let c = bytes[i];
            assert!(c.is_ascii_digit());
            val = val * 10 + (c - b'0') as u16;
            assert!(val <= u8::MAX as u16);
            i += 1;
        }
        val as u8
    }

    const fn parse_pre(bytes: &[u8]) -> u8 {
        if bytes.is_empty() {
            return 0;
        }
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_lowercase() {
            i += 1;
        }
        assert!(i > 0);
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
        }
        let pre = parse_u8(bytes, i);
        assert!(pre > 0);
        pre
    }

    FirmwareVersion {
        major: parse_u8(env!("CARGO_PKG_VERSION_MAJOR").as_bytes(), 0),
        minor: parse_u8(env!("CARGO_PKG_VERSION_MINOR").as_bytes(), 0),
        patch: parse_u8(env!("CARGO_PKG_VERSION_PATCH").as_bytes(), 0),
        pre: parse_pre(env!("CARGO_PKG_VERSION_PRE").as_bytes()),
    }
};
