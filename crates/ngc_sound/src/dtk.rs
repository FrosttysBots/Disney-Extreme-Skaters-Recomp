//! Streamed music (`.dtk`, the GameCube's disc-streaming ADPCM), as the
//! game keeps its songs and level ambiences (`music/dtk/...`).
//!
//! The file is just 32-byte blocks of 28 stereo samples at 48 kHz: bytes 0
//! and 1 give the left and right channels' predictor (high nibble) and
//! shift (low nibble), bytes 2 and 3 repeat them, and bytes 4 to 31 hold a
//! sample pair each, left in the low nibble and right in the high one. A
//! sample is the nibble shifted into the top of 16 bits and down by the
//! shift, plus a prediction from the last two (filters 0x3C;
//! 0x73, -0x34; 0x62, -0x37 in 64ths), kept at 6 extra bits.

/// The rate the disc streams at.
pub const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 32;
const PER_BLOCK: usize = 28;

/// One channel's last two samples (at 6 extra bits).
#[derive(Clone, Copy, Debug, Default)]
struct History(i32, i32);

impl History {
    fn decode(&mut self, nibble: u8, header: u8) -> i16 {
        let hist = match header >> 4 {
            1 => self.0 * 0x3C,
            2 => self.0 * 0x73 - self.1 * 0x34,
            3 => self.0 * 0x62 - self.1 * 0x37,
            _ => 0,
        };
        let hist = ((hist + 0x20) >> 6).clamp(-0x20_0000, 0x1F_FFFF);
        let shifted = i32::from(((u16::from(nibble) << 12) as i16) >> (header & 0xF));
        let current = (shifted << 6) + hist;
        self.1 = self.0;
        self.0 = current;
        (current >> 6).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }
}

/// Decodes a `.dtk` stream as it's played: interleaved left and right
/// samples, a block at a time.
#[derive(Clone, Debug)]
pub struct Dtk {
    data: Vec<u8>,
    block: usize,
    left: History,
    right: History,
    decoded: [i16; PER_BLOCK * 2],
    next: usize,
}

impl Dtk {
    pub fn new(data: Vec<u8>) -> Self {
        Dtk {
            data,
            block: 0,
            left: History::default(),
            right: History::default(),
            decoded: [0; PER_BLOCK * 2],
            next: PER_BLOCK * 2,
        }
    }

    /// How long it plays.
    pub fn seconds(&self) -> f32 {
        (self.data.len() / BLOCK * PER_BLOCK) as f32 / SAMPLE_RATE as f32
    }

    fn decode_block(&mut self) -> bool {
        let at = self.block * BLOCK;
        let Some(block) = self.data.get(at..at + BLOCK) else {
            return false;
        };
        for i in 0..PER_BLOCK {
            let byte = block[4 + i];
            self.decoded[i * 2] = self.left.decode(byte & 0xF, block[0]);
            self.decoded[i * 2 + 1] = self.right.decode(byte >> 4, block[1]);
        }
        self.block += 1;
        self.next = 0;
        true
    }
}

impl Iterator for Dtk {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        if self.next >= self.decoded.len() && !self.decode_block() {
            return None;
        }
        let sample = self.decoded[self.next];
        self.next += 1;
        Some(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_channels_of_a_block() {
        // No prediction, no shift: left nibble 1 -> 0x1000, right -1 -> -0x1000.
        let mut block = vec![0x00, 0x00, 0x00, 0x00];
        block.extend([0xF1u8; 28]);
        let samples: Vec<i16> = Dtk::new(block).collect();
        assert_eq!(samples.len(), 56);
        assert_eq!(&samples[..4], &[0x1000, -0x1000, 0x1000, -0x1000]);
    }

    #[test]
    fn shifts_and_predicts() {
        // Left: shift 4 (0x1000 >> 4 = 0x100), filter 1 (0x3C/64 of the last).
        let mut block = vec![0x14, 0x00, 0x14, 0x00];
        block.extend([0x01u8; 28]);
        let samples: Vec<i16> = Dtk::new(block).step_by(2).take(2).collect();
        assert_eq!(samples[0], 0x100);
        // 0x100 + 0x100 * 60/64 = 0x1F0.
        assert_eq!(samples[1], 0x1F0);
    }
}
