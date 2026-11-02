//! The game's cheat codes, as its Cheat Codes keyboard takes them
//! (`TryCheatString`, main.dol 0x80133758): each code is hashed to two
//! numbers, looked up in `Cheat_Array_Gamecube` (`{ c1 c2 CheatScript }`),
//! and the matching cheat's script run.
//!
//! The hash: a 128-byte buffer holds `12345678901234567890`, then the
//! code. A hundred thousand times, the checksum so far plus a count is
//! written there in decimal, its end marked `X` (the bytes after it left
//! as they were, up to the first zero), every byte added in times 1023,
//! and the buffer's checksum taken for the next round. `c1` is the
//! checksum of the code from a third of the way in, `c2` the buffer's,
//! each xored with the sum.

/// The game's checksum of bytes (CRC-32 of them lowercased, no final
/// xor), as `qb::checksum` is of text.
fn crc(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes.iter().map(u8::to_ascii_lowercase) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// What's in the buffer up to its first zero.
fn until_zero(buf: &[u8]) -> &[u8] {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    &buf[..end]
}

/// A code's two numbers (`c1`, `c2`), or `None` for one too short (the
/// game's take five letters or more).
pub fn cheat_hash(code: &str) -> Option<(i32, i32)> {
    let code = code.as_bytes();
    if code.len() <= 4 || code.len() >= 100 {
        return None;
    }
    let mut buf = [0xccu8; 128];
    let init = b"12345678901234567890\0";
    buf[..init.len()].copy_from_slice(init);
    buf[..code.len()].copy_from_slice(code);
    buf[code.len()] = 0;
    let mut sum = 0u32;
    let mut last = crc(code);
    for count in 0..100_000u32 {
        let digits = (count.wrapping_add(last) as i32).to_string();
        let digits = digits.as_bytes();
        buf[..digits.len()].copy_from_slice(digits);
        buf[digits.len()] = b'X';
        for &c in until_zero(&buf) {
            sum = sum.wrapping_add((i32::from(c as i8) * 1023) as u32);
        }
        last = crc(until_zero(&buf));
    }
    let c1 = crc(&code[code.len() / 3..]) ^ sum;
    let c2 = crc(until_zero(&buf)) ^ sum;
    Some((c1 as i32, c2 as i32))
}

#[cfg(test)]
mod tests {
    use super::cheat_hash;

    #[test]
    fn codes_hash_as_the_game_has_them() {
        // Cheat_Array_Gamecube's entries for the movie codes.
        assert_eq!(cheat_hash("marin"), Some((-1250178329, -1505971921)));
        assert_eq!(cheat_hash("nugget"), Some((-438145856, -109465049)));
        assert_eq!(cheat_hash("savannah"), Some((807457121, -841737808)));
        assert_eq!(cheat_hash("abc"), None);
    }
}
