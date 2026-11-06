//! Minimal Protocol Buffers wire codec (varint and length-delimited fields only), with strict
//! bounds checks. Enough for ShogiGUI `.sbk`; unknown fields are skipped.

use crate::{BookError, Result};

pub const VARINT: u8 = 0;
pub const I64: u8 = 1;
pub const LEN: u8 = 2;
pub const I32: u8 = 5;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

fn corrupt(m: &str) -> BookError {
    BookError::Corrupt(format!("protobuf: {m}"))
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }

    pub fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let b = *self.buf.get(self.pos).ok_or_else(|| corrupt("truncated varint"))?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(corrupt("varint longer than 10 bytes"))
    }

    /// (field number, wire type)
    pub fn key(&mut self) -> Result<(u32, u8)> {
        let k = self.varint()?;
        let field = u32::try_from(k >> 3).map_err(|_| corrupt("field number"))?;
        if field == 0 {
            return Err(corrupt("field number 0"));
        }
        Ok((field, (k & 7) as u8))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = usize::try_from(self.varint()?).map_err(|_| corrupt("length"))?;
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or_else(|| corrupt("length past end"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn string(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    /// int32 written as a (possibly 10-byte, sign-extended) varint.
    pub fn int32(&mut self) -> Result<i32> {
        Ok(self.varint()? as u32 as i32)
    }

    pub fn skip(&mut self, wire: u8) -> Result<()> {
        let n = match wire {
            VARINT => return self.varint().map(|_| ()),
            I64 => 8,
            LEN => return self.bytes().map(|_| ()),
            I32 => 4,
            _ => return Err(corrupt("unsupported wire type")),
        };
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or_else(|| corrupt("truncated field"))?;
        self.pos = end;
        Ok(())
    }

    /// Require `wire`, else error (protects against type confusion).
    pub fn expect(&self, got: u8, want: u8) -> Result<()> {
        if got == want { Ok(()) } else { Err(corrupt("unexpected wire type")) }
    }
}

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                return;
            }
            self.buf.push(b | 0x80);
        }
    }

    fn key(&mut self, field: u32, wire: u8) {
        self.varint(u64::from(field) << 3 | u64::from(wire));
    }

    /// int32 field; negative values are sign-extended to 10 bytes, as protobuf requires.
    pub fn int32(&mut self, field: u32, v: i32) {
        self.key(field, VARINT);
        self.varint(v as i64 as u64);
    }

    pub fn uint64(&mut self, field: u32, v: u64) {
        self.key(field, VARINT);
        self.varint(v);
    }

    pub fn bytes(&mut self, field: u32, b: &[u8]) {
        self.key(field, LEN);
        self.varint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn string(&mut self, field: u32, s: &str) {
        self.bytes(field, s.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_bounds() {
        let mut w = Writer::default();
        w.int32(1, -1);
        w.int32(2, 0x0100_7677);
        w.string(3, "abc");
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.key().unwrap(), (1, VARINT));
        assert_eq!(r.int32().unwrap(), -1);
        assert_eq!(r.key().unwrap(), (2, VARINT));
        assert_eq!(r.int32().unwrap(), 0x0100_7677);
        assert_eq!(r.key().unwrap(), (3, LEN));
        assert_eq!(r.string().unwrap(), "abc");
        assert!(r.done());
        // 10-byte encoding of -1 (as protobuf-net writes it).
        assert_eq!(&w.buf[1..11], &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        assert!(Reader::new(&[0x0a, 0xff, 0xff, 0xff, 0xff, 0x0f]).bytes().is_err());
        assert!(Reader::new(&[0x80; 11]).varint().is_err());
        assert!(Reader::new(&[0x00]).key().is_err());
    }
}
