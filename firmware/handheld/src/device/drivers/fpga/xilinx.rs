use std::io::Read;

use anyhow::bail;

#[allow(unused)]
mod consts {
    pub const TAG_DESIGN: u8 = 0x61;
    pub const TAG_PART: u8 = 0x62;
    pub const TAG_DATE: u8 = 0x63;
    pub const TAG_TIME: u8 = 0x64;
    pub const TAG_BITSTREAM: u8 = 0x65;
}

pub struct BitstreamMetadata {
    /// Bitstream payload length
    pub length: usize,
    /// Vivado UserID
    pub user_id: Option<u32>,
}

/// Parse the Xilinx .bit header from the file.
///
/// On success, leaves the cursor at the start of the bitstream payload.
pub fn parse_bitstream_header(f: &mut dyn Read) -> anyhow::Result<BitstreamMetadata> {
    fn read_u16(f: &mut dyn Read) -> anyhow::Result<u16> {
        let mut data = [0u8; 2];
        f.read_exact(&mut data)?;
        Ok(u16::from_be_bytes(data))
    }

    fn read_u32(f: &mut dyn Read) -> anyhow::Result<u32> {
        let mut data = [0u8; 4];
        f.read_exact(&mut data)?;
        Ok(u32::from_be_bytes(data))
    }

    // Read initial header
    const HEADER_LEN: usize = 9;
    let header_len = read_u16(f)?;
    if (header_len as usize) != HEADER_LEN {
        bail!("Unexpected header length");
    }
    let mut header = [0u8; HEADER_LEN];
    f.read_exact(&mut header)?;
    if header != [0x0F, 0xF0, 0x0F, 0xF0, 0x0F, 0xF0, 0x0F, 0xF0, 0x00] {
        bail!("Invalid header");
    }

    // Read the 2 bytes (0x0001)... a version perhaps?
    let unknown = read_u16(f)?;
    if unknown != 1 {
        bail!("Invalid unknown value");
    }

    let mut metadata = BitstreamMetadata {
        length: 0,
        user_id: None,
    };

    // Start reading tags.
    loop {
        let mut tag = [0u8; 1];
        f.read_exact(&mut tag)?;
        let tag = tag[0];

        if tag == consts::TAG_BITSTREAM {
            // Bitstream
            metadata.length = read_u32(f)? as usize;
            return Ok(metadata);
        }

        // Read and/or skip the tag
        let length = read_u16(f)? as usize;
        let mut num_read = 0;
        let mut buffer = [0u8; 128];
        while num_read < length {
            let amount = (length - num_read).min(buffer.len());
            f.read_exact(&mut buffer[0..amount])?;
            num_read += amount;
        }

        if length > buffer.len() {
            // We didn't get the full tag.
            continue;
        }

        let payload = &buffer[0..length];
        if tag == consts::TAG_DESIGN {
            // Find UserID field
            static PREFIX: &[u8] = b"UserID=";
            for c in payload.windows(PREFIX.len() + 8) {
                if c.starts_with(PREFIX) {
                    if let Ok(entry) = std::str::from_utf8(&c[PREFIX.len()..]) {
                        if let Ok(value) = u32::from_str_radix(entry, 16) {
                            metadata.user_id = Some(value);
                        }
                    }
                    break;
                }
            }
        }
    }
}
