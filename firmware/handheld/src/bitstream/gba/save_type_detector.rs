use gamebub_lib::bitstream::gba::SaveType;

/// Helper struct to auto-detect save file types from a ROM file (streaming)
pub struct SaveTypeDetector {
    detected: Option<SaveType>,
    buffer: Vec<u8>,
}

impl SaveTypeDetector {
    const OVERLAP: usize = 16;
    const STEP: usize = 4;

    pub fn new() -> Self {
        SaveTypeDetector {
            detected: None,
            buffer: vec![],
        }
    }

    pub fn get(&self) -> SaveType {
        self.detected.unwrap_or(SaveType::None)
    }

    fn search(data: &[u8]) -> Option<SaveType> {
        static PREFIXES: &[u32] = &[
            u32::from_ne_bytes(*b"EEPR"),
            u32::from_ne_bytes(*b"SRAM"),
            u32::from_ne_bytes(*b"FLAS"),
        ];
        static PATTERNS: &[(&[u8], SaveType)] = &[
            (b"EEPROM_V", SaveType::EepromAuto),
            (b"SRAM_V", SaveType::Sram),
            (b"SRAM_F_V", SaveType::Sram),
            (b"FLASH_V", SaveType::Flash64K),
            (b"FLASH512_V", SaveType::Flash64K),
            (b"FLASH1M_V", SaveType::Flash128K),
        ];

        // Turn the data into a [u32]
        let (data_prefix, data32, _) = unsafe {
            // SAFETY: it is ok to transmute u8 to u32
            data.align_to::<u32>()
        };
        assert!(data_prefix.is_empty());

        // First, do a high level search of each u32 word. If one of the
        // words is the prefix of one of the patterns, then do a full
        // comparison. This greatly improves the detection speed.
        for &prefix in PREFIXES {
            for i in 0..data32.len() {
                if data32[i] == prefix {
                    let region = &data[(i * 4)..];
                    for &(pattern, type_) in PATTERNS {
                        if region.starts_with(pattern) {
                            return Some(type_);
                        }
                    }
                }
            }
        }
        None
    }

    /// Process the next chunk of data.
    pub fn process(&mut self, data: &[u8]) {
        if self.detected.is_some() {
            return;
        }

        // Check the overlap of the last buffer to this buffer.
        let prefix = &data[..(data.len().min(Self::OVERLAP))];
        self.buffer.extend_from_slice(prefix);
        self.detected = Self::search(prefix);
        if self.detected.is_some() {
            return;
        }

        self.detected = Self::search(data);
        let suffix = &data[(data.len().saturating_sub(Self::OVERLAP) & !(Self::STEP - 1))..];
        self.buffer.clear();
        self.buffer.extend_from_slice(suffix);
    }
}
