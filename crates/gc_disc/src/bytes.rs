/// Reads a big-endian `u32`. The caller guarantees `buf` is long enough.
pub(crate) fn be_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}

/// Reads a NUL-terminated string, replacing invalid UTF-8.
pub(crate) fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}
