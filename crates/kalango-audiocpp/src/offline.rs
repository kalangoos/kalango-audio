//! Voice isolation of a whole recording: all the sound goes through the server as fast as it takes it (the model runs
//! about 7 times faster than the sound on a graphics card), nothing waits for real time.

use std::time::{Duration, Instant};

use crate::service::Service;
use crate::{Error, Result, SAMPLE_RATE};

/// What goes to the server in one push (20 ms: the model is faster on small blocks than on big ones, measured).
const BLOCK: usize = 960;
/// Silence after the end, so the model flushes its look-ahead; the result is cut back to the input's length.
const TAIL: usize = 8192;

pub struct Isolated {
    /// The isolated voice, mono, 48 kHz, as long as the input.
    pub samples: Vec<f32>,
    pub elapsed: Duration,
}

impl Isolated {
    /// Processing time over sound time (below 1 is faster than real time).
    pub fn rtf(&self) -> f32 {
        self.elapsed.as_secs_f32() / (self.samples.len().max(1) as f32 / SAMPLE_RATE as f32)
    }
}

/// Runs mono 48 kHz `input` through the voice isolator of a running `service`. `progress` gets the fraction done and
/// returns `false` to cancel.
pub fn isolate(service: &Service, input: &[f32], mut progress: impl FnMut(f32) -> bool) -> Result<Isolated> {
    let started = Instant::now();
    let mut stream = service.open_stream()?;
    let mut out: Vec<f32> = Vec::with_capacity(input.len() + TAIL);
    for (i, block) in input.chunks(BLOCK).enumerate() {
        stream.push(block)?;
        if let Some(part) = stream.pull(Duration::from_millis(0))? {
            out.extend(part);
        }
        if !progress(((i + 1) * BLOCK).min(input.len()) as f32 / input.len().max(1) as f32) {
            return Err(Error::Protocol("cancelled".into()));
        }
    }
    stream.push(&vec![0.0; TAIL])?;
    out.extend(stream.finish()?);
    out.resize(input.len(), 0.0);
    Ok(Isolated { samples: out, elapsed: started.elapsed() })
}

/// `amount` of the isolated voice plus the rest of the original: 1 is only the isolated voice, 0 only the original.
pub fn blend(original: &[f32], isolated: &[f32], amount: f32) -> Vec<f32> {
    let a = amount.clamp(0.0, 1.0);
    original.iter().zip(isolated).map(|(d, w)| a * w + (1.0 - a) * d).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_is_a_straight_mix() {
        assert_eq!(blend(&[1.0, 1.0], &[0.0, 0.5], 1.0), vec![0.0, 0.5]);
        assert_eq!(blend(&[1.0, 1.0], &[0.0, 0.5], 0.0), vec![1.0, 1.0]);
        assert_eq!(blend(&[1.0, 1.0], &[0.0, 0.5], 0.5), vec![0.5, 0.75]);
        assert_eq!(blend(&[1.0], &[0.0], 7.0), vec![0.0], "out-of-range amounts are clamped");
    }
}
