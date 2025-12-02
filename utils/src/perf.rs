use anyhow::{bail, Result};
use average::MeanWithError;
use std::time::{Duration, Instant};

pub struct Request {
    pub size: usize,
}

impl Request {
    pub fn to_bytes(&self) -> [u8; 8] {
        self.size.to_be_bytes()
    }
}

impl TryFrom<String> for Request {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        if value.len() < 4 || !value.ends_with("bit") {
            bail!("malformed blob size");
        }

        let bit_prefix = value.chars().rev().nth(3).unwrap();

        let size_bits = if bit_prefix.to_digit(10) == None {
            let mult: usize = match bit_prefix {
                'G' => 1000 * 1000 * 1000,
                'M' => 1000 * 1000,
                'K' => 1000,
                _ => bail!("unknown unit prefix"),
            };

            let size = value[..value.len() - 4].parse::<usize>()?;
            Ok(mult * size)
        } else {
            let size = value[..value.len() - 3].parse::<usize>()?;
            Ok(size)
        };

        size_bits.map(|k| Request { size: k / 8 })
    }
}

/// A blob represents the response payload
pub struct Blob {
    /// The size in bytes
    pub size: usize,

    /// The cursor indicating how much data has been
    /// sent so far
    pub cursor: usize,
}

impl From<[u8; 8]> for Blob {
    fn from(data: [u8; 8]) -> Self {
        let size = usize::from_be_bytes(data);

        Blob { size, cursor: 0 }
    }
}

impl TryFrom<&[u8]> for Blob {
    type Error = anyhow::Error;
    fn try_from(value: &[u8]) -> Result<Blob> {
        let value: [u8; 8] = value[0..8].try_into()?;
        Ok(Blob::from(value))
    }
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

    /// number of bytes transferred
    num_bytes: Vec<usize>,
}

impl Stats {
    pub fn new() -> Self {
        Stats {
            deltas: Vec::new(),
            start: None,
            num_bytes: Vec::new(),
        }
    }

    pub fn is_measuring(&self) -> bool {
        self.start.is_some()
    }

    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Starts a new measurement
    pub fn start_measurement(&mut self) {
        self.start = Some(Instant::now());
        self.num_bytes.push(0)
    }

    pub fn add_bytes(&mut self, num_bytes: usize) -> Result<usize> {
        let Some(val) = self.num_bytes.last_mut() else {
            bail!("haven't started a measurement yet.")
        };
        *val += num_bytes;
        Ok(*val)
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

            let num_bytes = self.num_bytes.last().unwrap();

            Ok((t, self.calculate_throughput(t, *num_bytes)))
        } else {
            bail!("haven't started a measurement yet.")
        }
    }

    pub fn durations(&self) -> MeanWithError {
        self.deltas.iter().map(|t| t.as_secs_f64()).collect()
    }

    pub fn throughputs(&self) -> MeanWithError {
        self.deltas
            .iter()
            .zip(self.num_bytes.iter())
            .map(|(t, b)| self.calculate_throughput(*t, *b))
            .collect()
    }

    #[inline]
    fn calculate_throughput(&self, t: Duration, b: usize) -> f64 {
        (b as f64) / 1000000.0 / t.as_secs_f64()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blob_sizes() {
        let req = Request::try_from(String::from("100Mbit")).expect("parse");
        assert_eq!(req.size, 100_000_000 / 8);

        let req = Request::try_from(String::from("100bit")).expect("parse");
        assert_eq!(req.size, 100 / 8);

        let req = Request::try_from(String::from("12lbit"));
        assert_eq!(req.is_err(), true);

        let req = Request::try_from(String::from("Gbit"));
        assert_eq!(req.is_err(), true);
    }

    #[test]
    fn create_responses() {
        let req = Request::try_from(String::from("20Gbit")).expect("parse");
        let res = Blob::from(req.to_bytes());
        assert_eq!(req.size, res.size);
    }
}
