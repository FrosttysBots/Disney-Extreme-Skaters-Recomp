//! Nintendo's DSP ADPCM sounds (`.dsp`), as the game keeps its sound
//! effects (`sounds/dsp/...` in the `.prg` archives).
//!
//! ```text
//! 0x00 u32  samples
//! 0x04 u32  nibbles
//! 0x08 u32  sample rate
//! 0x0C u16  looped (1)
//! 0x0E u16  format (0: ADPCM)
//! 0x10 u32  loop start, 0x14 loop end (nibble addresses)
//! 0x18 u32  current address
//! 0x1C i16 x 16  coefficients (8 predictor pairs)
//! 0x3C u16  gain, 0x3E initial predictor/scale, 0x40 i16 history 1, 2
//! 0x44 ...  loop predictor/scale and history, padding to 0x60
//! 0x60 ...  frames of 8 bytes: a predictor/scale byte, then 14 nibbles
//! ```
//!
//! All big-endian. Each nibble is a signed 4-bit value; a sample is
//! `(nibble * scale << 11) + 1024 + coef1 * hist1 + coef2 * hist2 >> 11`,
//! clamped to 16 bits.

pub mod dtk;

pub use dtk::Dtk;

/// What went wrong reading a sound.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the file is too short ({0} bytes)")]
    Truncated(usize),
    #[error("unsupported format {0}")]
    Format(u16),
}

/// A decoded mono sound.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub sample_rate: u32,
    pub samples: Vec<i16>,
    /// Where it loops back to, if it loops (a sample index).
    pub loop_start: Option<usize>,
}

const HEADER: usize = 0x60;
const FRAME: usize = 8;
const PER_FRAME: usize = 14;

impl Sound {
    /// Decodes a `.dsp` file.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.len() < HEADER {
            return Err(Error::Truncated(data.len()));
        }
        let u32_at = |at: usize| u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
        let u16_at = |at: usize| u16::from_be_bytes(data[at..at + 2].try_into().unwrap());
        let samples = u32_at(0) as usize;
        let sample_rate = u32_at(8);
        let looped = u16_at(0x0C) != 0;
        let format = u16_at(0x0E);
        if format != 0 {
            return Err(Error::Format(format));
        }
        let loop_start = u32_at(0x10) as usize;
        let coef: Vec<i32> = (0..16)
            .map(|i| i32::from(u16_at(0x1C + i * 2) as i16))
            .collect();
        let (mut hist1, mut hist2) = (
            i32::from(u16_at(0x40) as i16),
            i32::from(u16_at(0x42) as i16),
        );
        let mut out = Vec::with_capacity(samples);
        for frame in data[HEADER..].chunks(FRAME) {
            if out.len() >= samples || frame.is_empty() {
                break;
            }
            let ps = frame[0];
            let predictor = usize::from((ps >> 4) & 7);
            let scale = 1i32 << (ps & 0xF);
            let (c1, c2) = (coef[predictor * 2], coef[predictor * 2 + 1]);
            for &byte in &frame[1..] {
                for nibble in [byte >> 4, byte & 0xF] {
                    if out.len() >= samples {
                        break;
                    }
                    // Sign-extend the nibble.
                    let n = i32::from(((nibble << 4) as i8) >> 4);
                    let sample = ((n * scale) << 11) + 1024 + c1 * hist1 + c2 * hist2;
                    let sample = (sample >> 11).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                    hist2 = hist1;
                    hist1 = sample;
                    out.push(sample as i16);
                }
            }
        }
        // A nibble address: two header nibbles start each frame.
        let loop_start = looped.then(|| {
            let frame = loop_start / (FRAME * 2);
            let within = (loop_start % (FRAME * 2)).saturating_sub(2);
            (frame * PER_FRAME + within).min(out.len().saturating_sub(1))
        });
        Ok(Sound {
            sample_rate,
            samples: out,
            loop_start,
        })
    }

    /// The samples as floats from -1 to 1.
    pub fn floats(&self) -> Vec<f32> {
        self.samples
            .iter()
            .map(|&s| f32::from(s) / 32768.0)
            .collect()
    }

    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate.max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(samples: u32, rate: u32, looped: bool, coef: [i16; 16]) -> Vec<u8> {
        let mut h = vec![0u8; HEADER];
        h[0..4].copy_from_slice(&samples.to_be_bytes());
        h[4..8].copy_from_slice(&(samples / 14 * 16 + 2).to_be_bytes());
        h[8..12].copy_from_slice(&rate.to_be_bytes());
        h[0x0C..0x0E].copy_from_slice(&u16::from(looped).to_be_bytes());
        // Loop from the second frame's first sample (nibble address 18).
        h[0x10..0x14].copy_from_slice(&18u32.to_be_bytes());
        for (i, c) in coef.iter().enumerate() {
            h[0x1C + i * 2..0x1E + i * 2].copy_from_slice(&c.to_be_bytes());
        }
        h
    }

    #[test]
    fn decodes_nibbles_with_the_scale() {
        // No prediction: each sample is nibble * scale (rounded).
        let mut data = header(14, 32000, false, [0; 16]);
        // Predictor 0, scale 1 << 4 = 16; nibbles 1, -1, 7, -8, then zeros.
        data.extend_from_slice(&[0x04, 0x1F, 0x78, 0, 0, 0, 0, 0]);
        let sound = Sound::parse(&data).unwrap();
        assert_eq!(sound.sample_rate, 32000);
        assert_eq!(&sound.samples[..5], &[16, -16, 112, -128, 0]);
        assert_eq!(sound.samples.len(), 14);
        assert_eq!(sound.loop_start, None);
    }

    #[test]
    fn predicts_from_the_last_two_samples() {
        // Predictor 0 with coef1 = 2048 (1.0): each sample adds to the last.
        let mut coef = [0i16; 16];
        coef[0] = 2048;
        let mut data = header(28, 22050, true, coef);
        data.extend_from_slice(&[0x00, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]);
        data.extend_from_slice(&[0x00, 0, 0, 0, 0, 0, 0, 0]);
        let sound = Sound::parse(&data).unwrap();
        assert_eq!(&sound.samples[..3], &[1, 2, 3]);
        assert_eq!(sound.samples[13], 14);
        assert_eq!(sound.samples[27], 14, "holds without new nibbles");
        assert_eq!(sound.loop_start, Some(14));
    }

    #[test]
    fn rejects_short_files() {
        assert!(matches!(Sound::parse(&[0; 10]), Err(Error::Truncated(10))));
    }
}
