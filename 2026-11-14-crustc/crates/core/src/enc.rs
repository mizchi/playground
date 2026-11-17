//! キャッシュ用の小さなバイナリ符号化。要約 (Summary) は全アイテム分がメモリとディスクに載るので、
//! 空のフィールドは書かない (先頭のビットマスクで有無を表す) ようにして小さく保つ。

pub trait Enc: Sized {
    fn enc(&self, out: &mut Vec<u8>);
    fn dec(r: &mut Rd) -> Option<Self>;
    /// 空 (= 書かなくてよい) か
    fn is_empty_val(&self) -> bool {
        false
    }
}

pub struct Rd<'a> {
    pub d: &'a [u8],
    pub pos: usize,
}

impl<'a> Rd<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Rd { d, pos: 0 }
    }
    pub fn u8(&mut self) -> Option<u8> {
        let b = *self.d.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    pub fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
}

pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

impl Enc for bool {
    fn enc(&self, out: &mut Vec<u8>) {
        out.push(*self as u8)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(r.u8()? != 0)
    }
    fn is_empty_val(&self) -> bool {
        !*self
    }
}

impl Enc for u64 {
    fn enc(&self, out: &mut Vec<u8>) {
        put_varint(out, *self)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        r.varint()
    }
    fn is_empty_val(&self) -> bool {
        *self == 0
    }
}

impl Enc for i64 {
    fn enc(&self, out: &mut Vec<u8>) {
        // zigzag
        put_varint(out, ((*self << 1) ^ (*self >> 63)) as u64)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        let v = r.varint()?;
        Some(((v >> 1) as i64) ^ -((v & 1) as i64))
    }
    fn is_empty_val(&self) -> bool {
        *self == 0
    }
}

impl Enc for String {
    fn enc(&self, out: &mut Vec<u8>) {
        put_varint(out, self.len() as u64);
        out.extend_from_slice(self.as_bytes());
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        let n = r.varint()? as usize;
        String::from_utf8(r.take(n)?.to_vec()).ok()
    }
    fn is_empty_val(&self) -> bool {
        self.is_empty()
    }
}

impl<T: Enc> Enc for Vec<T> {
    fn enc(&self, out: &mut Vec<u8>) {
        put_varint(out, self.len() as u64);
        for x in self {
            x.enc(out);
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        let n = r.varint()? as usize;
        (0..n).map(|_| T::dec(r)).collect()
    }
    fn is_empty_val(&self) -> bool {
        self.is_empty()
    }
}

impl<T: Enc> Enc for Option<T> {
    fn enc(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                v.enc(out);
            }
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        match r.u8()? {
            0 => Some(None),
            _ => Some(Some(T::dec(r)?)),
        }
    }
    fn is_empty_val(&self) -> bool {
        self.is_none()
    }
}

impl<A: Enc, B: Enc> Enc for (A, B) {
    fn enc(&self, out: &mut Vec<u8>) {
        self.0.enc(out);
        self.1.enc(out);
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some((A::dec(r)?, B::dec(r)?))
    }
}

impl<A: Enc, B: Enc, C: Enc> Enc for (A, B, C) {
    fn enc(&self, out: &mut Vec<u8>) {
        self.0.enc(out);
        self.1.enc(out);
        self.2.enc(out);
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some((A::dec(r)?, B::dec(r)?, C::dec(r)?))
    }
}

/// struct の Enc を「空でないフィールドのビットマスク + そのフィールドだけ」で実装する
#[macro_export]
macro_rules! enc_struct {
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl $crate::enc::Enc for $t {
            fn enc(&self, out: &mut Vec<u8>) {
                let mut mask: u64 = 0;
                let mut bit = 1u64;
                $( if !$crate::enc::Enc::is_empty_val(&self.$f) { mask |= bit; } bit <<= 1; )*
                let _ = bit;
                $crate::enc::put_varint(out, mask);
                let mut bit = 1u64;
                $( if mask & bit != 0 { $crate::enc::Enc::enc(&self.$f, out); } bit <<= 1; )*
                let _ = bit;
            }
            fn dec(r: &mut $crate::enc::Rd) -> Option<Self> {
                let mask = r.varint()?;
                let mut v = <$t>::default();
                let mut bit = 1u64;
                $( if mask & bit != 0 { v.$f = $crate::enc::Enc::dec(r)?; } bit <<= 1; )*
                let _ = bit;
                Some(v)
            }
        }
    };
}
