use embedded_io::{Read, ReadExactError};

#[allow(unused)]
mod consts {
    pub const TAG_DESIGN: u8 = 0x61;
    pub const TAG_PART: u8 = 0x62;
    pub const TAG_DATE: u8 = 0x63;
    pub const TAG_TIME: u8 = 0x64;
    pub const TAG_BITSTREAM: u8 = 0x65;
}

const HEADER_LEN: usize = 9;

pub struct BitstreamMetadata {
    /// Bitstream payload length
    pub length: usize,
    /// Vivado UserID
    pub user_id: Option<u32>,
}

/// Why a header failed to parse.
///
/// Concrete rather than `anyhow`, which would need the reader's error to be
/// `Send + Sync + 'static` -- a bound `embedded_io::Read` never promises, and
/// one that spreads to every caller until it reaches a reader too opaque to
/// name it on.
#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("bitstream ended mid-header")]
    UnexpectedEof,
    #[error("could not read bitstream")]
    Io,
    #[error("header length field was not {HEADER_LEN}")]
    BadHeaderLength,
    #[error("header magic did not match")]
    BadMagic,
    #[error("version field was not 1")]
    BadVersion,
}

// Blanket over the reader's error type, so `?` needs no bound on it at all.
impl<E> From<ReadExactError<E>> for HeaderError {
    fn from(e: ReadExactError<E>) -> Self {
        match e {
            ReadExactError::UnexpectedEof => Self::UnexpectedEof,
            ReadExactError::Other(_) => Self::Io,
        }
    }
}

/// Parse the Xilinx .bit header from the file.
///
/// On success, leaves the cursor at the start of the bitstream payload.
pub fn parse_bitstream_header<R: Read>(f: &mut R) -> Result<BitstreamMetadata, HeaderError> {
    fn read_u16<R: Read>(f: &mut R) -> Result<u16, HeaderError> {
        let mut data = [0u8; 2];
        f.read_exact(&mut data)?;
        Ok(u16::from_be_bytes(data))
    }

    fn read_u32<R: Read>(f: &mut R) -> Result<u32, HeaderError> {
        let mut data = [0u8; 4];
        f.read_exact(&mut data)?;
        Ok(u32::from_be_bytes(data))
    }

    // Read initial header
    let header_len = read_u16(f)?;
    if (header_len as usize) != HEADER_LEN {
        return Err(HeaderError::BadHeaderLength);
    }
    let mut header = [0u8; HEADER_LEN];
    f.read_exact(&mut header)?;
    if header != [0x0F, 0xF0, 0x0F, 0xF0, 0x0F, 0xF0, 0x0F, 0xF0, 0x00] {
        return Err(HeaderError::BadMagic);
    }

    // Read the 2 bytes (0x0001)... a version perhaps?
    let unknown = read_u16(f)?;
    if unknown != 1 {
        return Err(HeaderError::BadVersion);
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
        match tag {
            consts::TAG_DESIGN => {
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
            _ => {}
        }
    }
}
