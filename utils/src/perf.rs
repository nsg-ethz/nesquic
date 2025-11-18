use anyhow::{bail, Result};
use average::MeanWithError;
use std::{
    str,
    time::{Duration, Instant},
};

/// A blob represents the response payload
pub struct Blob {
    /// The size in bytes
    pub size: u64,

    /// The cursor indicating how much data has been
    /// sent so far
    pub cursor: u64,
}

impl Iterator for Blob {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor < self.size {
            self.cursor += 1;
            Some(0)
        } else {
            None
        }
    }
}

/// Stats keep track of the measurements
#[derive(Clone, Debug)]
pub struct Stats {
    /// the measured time differences
    deltas: Vec<Duration>,

    /// the current start time
    start: Option<Instant>,

    /// reference blob size in bytes
    /// needed to compute throughput
    blob_size: u64,
}

impl Stats {
    pub fn new(blob_bytes: u64) -> Self {
        Stats {
            deltas: Vec::new(),
            start: None,
            blob_size: blob_bytes,
        }
    }

    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Starts a new measurement
    pub fn start_measurement(&mut self) {
        self.start = Some(Instant::now());
    }

    /// Stops the current measurement and resets the current measurement
    /// Returns an error if `start_measurement` wasn't called before
    /// Otherwise, returns measured duration and throughput in Mbit/s
    pub fn stop_measurement(&mut self) -> Result<(Duration, f64)> {
        if let Some(start) = self.start {
            let end = Instant::now();
            let t = end - start;
            self.deltas.push(t);
            self.start = None;

            Ok((t, self.throughput_for_duration(&t)))
        } else {
            bail!("haven't started a measurement yet.")
        }
    }

    pub fn durations(&self) -> MeanWithError {
        self.deltas.iter().map(|t| t.as_secs_f64()).collect()
    }

    fn throughput_for_duration(&self, t: &Duration) -> f64 {
        self.blob_size as f64 / (t.as_secs_f64() * 1000.0 * 1000.0)
    }

    pub fn throughputs(&self) -> MeanWithError {
        self.deltas
            .iter()
            .map(|t| self.throughput_for_duration(t))
            .collect()
    }

    /// Summarizes the measurements
    pub fn summary(&self) -> String {
        let ts = self.durations();
        let tp = self.throughputs();

        format!(
            "reps: {} duration: {:.5}s +- {:.5}s
        mean throughput: {:.5}Mbit/s std throughput: {:.5}Mbit/s",
            self.deltas.len(),
            ts.mean(),
            ts.error(),
            tp.mean(),
            tp.error()
        )
    }
}

pub fn create_req(blob: &str) -> Result<[u8; 8]> {
    let size = parse_blob_size(blob)?;
    Ok(size.to_be_bytes())
}

pub fn process_req(buf: &[u8]) -> Result<Blob> {
    let size = u64::from_be_bytes(buf.try_into()?);

    Ok(Blob { size, cursor: 0 })
}

/// Parses blob sizes specified in [GMK]bit
/// Returns the specified number of bytes
pub fn parse_blob_size(value: &str) -> Result<u64> {
    if value.len() < 4 || !value.ends_with("bit") {
        bail!("malformed blob size");
    }

    let bit_prefix = value.chars().rev().nth(3).unwrap();

    let size_bits = if bit_prefix.to_digit(10) == None {
        let mult: u64 = match bit_prefix {
            'G' => 1000 * 1000 * 1000,
            'M' => 1000 * 1000,
            'K' => 1000,
            _ => bail!("unknown unit prefix"),
        };

        let size = value[..value.len() - 4].parse::<u64>()?;
        Ok(mult * size)
    } else {
        let size = value[..value.len() - 3].parse::<u64>()?;
        Ok(size)
    };

    size_bits.map(|k| k / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blob_sizes() {
        let size = parse_blob_size("100Mbit");
        assert_eq!(size.unwrap(), 100_000_000 / 8);

        let size = parse_blob_size("100bit");
        assert_eq!(size.unwrap(), 100 / 8);

        let size = parse_blob_size("12lbit");
        assert_eq!(size.is_err(), true);

        let size = parse_blob_size("Gbit");
        assert_eq!(size.is_err(), true);
    }

    #[test]
    fn create_requests() {
        let blob = "20Gbit";
        let lhs = process_req(&create_req(&blob).unwrap()).unwrap().size;
        let rhs = parse_blob_size(&blob).unwrap();
        assert_eq!(lhs, rhs);
    }
}
