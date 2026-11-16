//! キャッシュキー (128 bit のハッシュ)。cache.rs と program.rs が使う。
//!
//! 以前は SipHash (std の DefaultHasher) を 2 つ使っていたが、全アイテムの全トークンを毎ビルドハッシュするので、
//! crustc でコンパイルしたコンパイラ (インライン展開がない) では目に見えて遅かった。
//! 2 本の 64 bit の掛け算・回転の混ぜ合わせ (互いに違う定数) を使い、最後に murmur3 の fmix64 で混ぜる。
//! 暗号的な強さは要らない (入力は自分のソース) が、128 bit なので偶然の衝突は起きない。

use std::hash::{Hash, Hasher};

pub type Key = u128;

/// バイト列を 8 byte ずつ混ぜる。呼び出しの区切りに依らない (write("ab") と write("a"); write("b") が同じ):
/// std と crustc の prelude で Hash の実装が write を呼ぶ区切りが違っても同じ値になるように
#[derive(Clone, Copy)]
pub struct KeyState {
    a: u64,
    b: u64,
    /// まだ混ぜていないバイト (下位から) とその数、全体の長さ
    buf: u64,
    nb: u32,
    len: u64,
}

const K1: u64 = 0x9E37_79B9_7F4A_7C15;
const K2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const K3: u64 = 0x1656_67B1_9E37_79F9;

impl KeyState {
    #[inline]
    fn word(&mut self, w: u64) {
        self.a = (self.a ^ w).wrapping_mul(K1).rotate_left(29);
        self.b = (self.b.rotate_left(23) ^ w.wrapping_mul(K3)).wrapping_mul(K2);
    }
    /// 残りのバイトと長さを混ぜた状態
    fn done(&self) -> (u64, u64) {
        let mut s = *self;
        s.word(s.buf ^ (s.nb as u64) << 59);
        s.word(s.len);
        (s.a, s.b)
    }
}

fn fmix(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    k ^= k >> 33;
    k
}

impl Hasher for KeyState {
    fn write(&mut self, bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);
        let mut i = 0;
        // (途中のバイトがあれば 1 byte ずつ足して 8 byte にする)
        while self.nb != 0 && i < bytes.len() {
            self.buf |= (bytes[i] as u64) << (8 * self.nb);
            self.nb += 1;
            i += 1;
            if self.nb == 8 {
                let w = self.buf;
                self.word(w);
                self.buf = 0;
                self.nb = 0;
            }
        }
        while i + 8 <= bytes.len() {
            let mut w = 0u64;
            let mut j = 0;
            while j < 8 {
                w |= (bytes[i + j] as u64) << (8 * j);
                j += 1;
            }
            self.word(w);
            i += 8;
        }
        while i < bytes.len() {
            self.buf |= (bytes[i] as u64) << (8 * self.nb);
            self.nb += 1;
            i += 1;
        }
    }
    fn finish(&self) -> u64 {
        let (a, b) = self.done();
        fmix(a ^ fmix(b))
    }
}

pub struct KeyHasher(KeyState);

impl KeyHasher {
    pub fn new() -> Self {
        KeyHasher(KeyState { a: 0x243F_6A88_85A3_08D3, b: 0x1319_8A2E_0370_7344, buf: 0, nb: 0, len: 0 })
    }
    pub fn add<T: Hash + ?Sized>(&mut self, v: &T) {
        v.hash(&mut self.0);
    }
    pub fn finish(&self) -> Key {
        let (a, b) = self.0.done();
        let hi = fmix(a ^ fmix(b));
        let lo = fmix(b.wrapping_add(K1) ^ fmix(a.rotate_left(17)));
        ((hi as u128) << 64) | lo as u128
    }
}
