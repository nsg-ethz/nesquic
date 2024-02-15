use std::str;
use anyhow::{Result, Context, bail};

pub mod args;

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

pub fn parse_bit_size(value: &str) -> Result<u64> {
    if value.len() < 5 || !value.starts_with("/") || !value.ends_with("bit") {
        bail!("malformed blob size");
    }

    let bit_prefix = value
        .chars()
        .rev()
        .nth(3)
        .unwrap();
    if bit_prefix.to_digit(10) == None {
        let mult: u64 = match bit_prefix {
            'G' => 1000 * 1000 * 1000,
            'M' => 1000 * 1000,
            'K' => 1000,
            _ => bail!("unknown unit prefix")
        };

        let size = value[1..value.len()-4].parse::<u64>()?;
        Ok(mult * size)
    }
    else {
        let size = value[1..value.len()-3].parse::<u64>()?;
        Ok(size)
    }
} 

pub fn process_get(x: &[u8]) -> Result<Blob> {
    if x.len() < 4 || &x[0..4] != b"GET " {
        bail!("missing GET");
    }
    if x[4..].len() < 2 || &x[x.len() - 2..] != b"\r\n" {
        bail!("missing \\r\\n");
    }
    let x = &x[4..x.len() - 2];
    let end = x.iter().position(|&c| c == b' ').unwrap_or(x.len());
    let path = str::from_utf8(&x[..end]).context("path is malformed UTF-8")?;
    let bsize = parse_bit_size(path)?;

    Ok(Blob {
        size: bsize / 8,
        cursor: 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
