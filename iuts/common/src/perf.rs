use std::str;
use anyhow::{Result, Context, bail};

/// A blob represents the response payload
pub struct Blob {
    /// The size in bytes
    pub size: u64,

    /// The cursor indicating how much data has been
    /// sent so far
    pub cursor: u64
}

impl Iterator for Blob {

    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor < self.size {
            self.cursor += 1;
            Some(0)
        }   
        else {
            None
        }
    }

}

pub fn create_req(blob: &str) -> Result<[u8; 8]> {
    let size = parse_blob_size(blob)?;
    Ok(size.to_be_bytes())
}

pub fn process_req(buf: &[u8]) -> Result<Blob> {
    let size = u64::from_be_bytes(buf.try_into()?);

    Ok(Blob {
        size,
        cursor: 0
    })
}

/// Parses blob sizes specified in [GMK]bit
/// Returns the specified number of bytes
pub fn parse_blob_size(value: &str) -> Result<u64> {
    if value.len() < 4 || !value.ends_with("bit") {
        bail!("malformed blob size");
    }

    let bit_prefix = value
        .chars()
        .rev()
        .nth(3)
        .unwrap();
    
    
    let size_bits = if bit_prefix.to_digit(10) == None {
        let mult: u64 = match bit_prefix {
            'G' => 1000 * 1000 * 1000,
            'M' => 1000 * 1000,
            'K' => 1000,
            _ => bail!("unknown unit prefix")
        };

        let size = value[..value.len()-4].parse::<u64>()?;
        Ok(mult * size)
    }
    else {
        let size = value[..value.len()-3].parse::<u64>()?;
        Ok(size)
    };

    size_bits.map(|k| k/8)
} 

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_bit_size() {
        let size = parse_blob_size("100Mbit");
        assert_eq!(size.unwrap(), 100_000_000/8);
    }
}
