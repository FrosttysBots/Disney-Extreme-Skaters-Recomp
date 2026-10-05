use crate::{Error, Result};

/// A big-endian cursor that reports where truncation happened.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pub offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    pub fn bytes(&mut self, len: usize, what: &str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|&end| end <= self.data.len());
        let Some(end) = end else {
            return Err(Error::Truncated(format!("{what} at {:#x}", self.offset)));
        };
        let out = &self.data[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    pub fn u32(&mut self, what: &str) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4, what)?.try_into().unwrap()))
    }

    pub fn i32(&mut self, what: &str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }

    pub fn f32(&mut self, what: &str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    pub fn f32s<const N: usize>(&mut self, what: &str) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for value in &mut out {
            *value = self.f32(what)?;
        }
        Ok(out)
    }

    /// Reads a count and rejects values that can't fit in the rest of the
    /// file, so garbage never triggers a huge allocation.
    pub fn count(&mut self, what: &str, min_item_size: usize) -> Result<usize> {
        let at = self.offset;
        let count = self.u32(what)? as usize;
        if count.saturating_mul(min_item_size) > self.remaining() {
            return Err(Error::Invalid(format!(
                "{what} {count} at {at:#x} is larger than the file"
            )));
        }
        Ok(count)
    }
}
