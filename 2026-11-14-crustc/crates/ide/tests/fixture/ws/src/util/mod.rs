pub mod text;

pub use self::text::Word;

/// Scale a value.
pub fn scale(v: f64) -> f64 {
    v * FACTOR
}

const FACTOR: f64 = 1.5;

pub struct Wrapper<T> {
    pub inner: T,
}

impl<T: Clone> Wrapper<T> {
    pub fn get(&self) -> T {
        self.inner.clone()
    }
}

pub fn wrapped() -> i32 {
    let w = Wrapper { inner: super::shapes::Point::new(1, 1) };
    let p = w.get();
    let k = w.inner.x;
    p.y + k
}
