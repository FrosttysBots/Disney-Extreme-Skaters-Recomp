//! LZSS decompression in the classic Okumura layout.
//!
//! The compressed stream is a series of groups: one flag byte, then eight
//! items read from its lowest bit up. A set bit is a literal byte. A clear bit
//! is a two-byte back-reference into a 4 KiB ring buffer: 12 bits of position
//! and 4 bits of length (3..=18 bytes).
//!
//! The ring buffer starts zero-filled and writing begins at `RING_SIZE - MAX_MATCH`.
//! Real archives do reference the untouched zeros, so the fill value matters.

const RING_SIZE: usize = 4096;
const RING_MASK: usize = RING_SIZE - 1;
const MAX_MATCH: usize = 18;
const MIN_MATCH: usize = 3;

/// The compressed stream ended before producing the expected output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncatedInput;

pub fn decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>, TruncatedInput> {
    let mut ring = [0u8; RING_SIZE];
    let mut r = RING_SIZE - MAX_MATCH;
    let mut out = Vec::with_capacity(out_len);
    let mut input = src.iter().copied();
    let mut next = || input.next().ok_or(TruncatedInput);

    // The high byte tracks how many flag bits remain in the current group.
    let mut flags: u32 = 0;
    while out.len() < out_len {
        flags >>= 1;
        if flags & 0x100 == 0 {
            flags = u32::from(next()?) | 0xFF00;
        }

        if flags & 1 != 0 {
            let byte = next()?;
            out.push(byte);
            ring[r] = byte;
            r = (r + 1) & RING_MASK;
        } else {
            let lo = next()?;
            let hi = next()?;
            let position = usize::from(lo) | (usize::from(hi & 0xF0) << 4);
            let length = usize::from(hi & 0x0F) + MIN_MATCH;
            for k in 0..length {
                let byte = ring[(position + k) & RING_MASK];
                out.push(byte);
                ring[r] = byte;
                r = (r + 1) & RING_MASK;
            }
        }
    }

    // A final back-reference can run past the end.
    out.truncate(out_len);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_only() {
        assert_eq!(decompress(&[0xFF, b'h', b'i'], 2).unwrap(), b"hi");
    }

    #[test]
    fn overlapping_back_reference() {
        // Three literals, then copy 6 bytes from where the first literal was
        // written (4078 = 0xFEE): the copy reads bytes it is itself producing.
        let stream = [0b0000_0111, b'a', b'b', b'c', 0xEE, 0xF0 | (6 - 3) as u8];
        assert_eq!(decompress(&stream, 9).unwrap(), b"abcabcabc");
    }

    #[test]
    fn reference_into_initial_zeros() {
        // A back-reference to position 0 before anything was written there.
        assert_eq!(decompress(&[0x00, 0x00, 0x00], 3).unwrap(), [0, 0, 0]);
    }

    #[test]
    fn output_is_cut_to_requested_length() {
        let stream = [0b0000_0111, b'a', b'b', b'c', 0xEE, 0xF0 | (6 - 3) as u8];
        assert_eq!(decompress(&stream, 5).unwrap(), b"abcab");
    }

    #[test]
    fn truncated_stream_is_an_error() {
        assert_eq!(decompress(&[0xFF, b'a'], 2), Err(TruncatedInput));
        assert_eq!(decompress(&[0x00, 0xEE], 3), Err(TruncatedInput));
        assert_eq!(decompress(&[], 1), Err(TruncatedInput));
    }
}
