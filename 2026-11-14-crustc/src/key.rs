//! キャッシュキー (128 bit のハッシュ)。cache.rs と program.rs が使う。

use std::hash::{Hash, Hasher};

pub type Key = u128;

pub struct KeyHasher(std::hash::DefaultHasher, std::hash::DefaultHasher);

impl KeyHasher {
    pub fn new() -> Self {
        // DefaultHasher::new() は固定鍵の SipHash で、同じ std なら決定的。
        // std のバージョン差はコンパイラ ID (実行ファイル自体) をキーに含めることで吸収する。
        let a = std::hash::DefaultHasher::new();
        let mut b = std::hash::DefaultHasher::new();
        0xa5a5_u16.hash(&mut b);
        KeyHasher(a, b)
    }
    pub fn add<T: Hash + ?Sized>(&mut self, v: &T) {
        v.hash(&mut self.0);
        v.hash(&mut self.1);
    }
    pub fn finish(&self) -> Key {
        ((self.0.finish() as u128) << 64) | self.1.finish() as u128
    }
}

