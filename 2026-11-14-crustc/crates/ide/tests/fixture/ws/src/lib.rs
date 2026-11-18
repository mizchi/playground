//! Fixture crate for the ide golden tests.
#![allow(dead_code, unused)]

pub mod shapes;
pub mod util;

mod inner {
    pub fn deep() -> u32 {
        7
    }
    pub mod more {
        /// A hidden thing.
        pub struct Hidden {
            pub v: i32,
        }
        impl Hidden {
            pub fn get(&self) -> i32 {
                self.v
            }
        }
    }
}

use crate::shapes::{area, Point as Pt, Shape};
use std::collections::HashMap;
use std::sync::Mutex;
use util::text::*;

pub use inner::more::Hidden;

/// A counter type.
/// It counts things.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counter {
    pub count: u64,
    pub name: String,
    pub items: Vec<Pt>,
}

pub type Pair<T> = (T, T);
pub type Res<T> = Result<T, Error>;
pub type Points = Vec<Pt>;
pub type Table<V> = HashMap<String, V>;

#[derive(Debug)]
pub enum Error {
    Empty,
    Bad(String),
    Code { n: i32 },
}

pub const LIMIT: usize = 16;
pub static ORIGIN: Pt = Pt { x: 0, y: 0 };
static mut HITS: u32 = 0;

pub trait Describe {
    fn describe(&self) -> String;
    fn twice(&self) -> String {
        let s = self.describe();
        format!("{s}{s}")
    }
}

impl Describe for Counter {
    fn describe(&self) -> String {
        self.name.clone()
    }
}

impl Counter {
    pub fn new(name: &str) -> Self {
        Self { count: 0, name: name.to_string(), ..Default::default() }
    }

    pub fn bump(&mut self) -> &mut Self {
        self.count += 1;
        self
    }

    pub fn first(&self) -> Option<&Pt> {
        self.items.first()
    }

    pub fn total(&self) -> Res<i64> {
        if self.items.is_empty() {
            return Err(Error::Empty);
        }
        let mut sum = 0i64;
        for p in self.items.iter() {
            sum += p.x as i64;
        }
        for (i, q) in self.items.iter().enumerate() {
            let _ = (i, q.y);
        }
        Ok(sum)
    }

    pub fn points(&self) -> Points {
        self.items.clone()
    }

    fn checked(&self) -> Res<u64> {
        let t = self.total()?;
        let first = self.first().unwrap();
        let n = first.x;
        Ok(t as u64 + n as u64 + self.count)
    }
}

pub fn make_pair(a: i64) -> Pair<i64> {
    (a, a + 1)
}

pub fn tally(words: &[&str]) -> Table<usize> {
    let mut t: Table<usize> = HashMap::new();
    for w in words {
        let k = w.trim().to_string();
        let _old = t.insert(k, 1);
    }
    if let Some(v) = t.get(&String::new()) {
        let _ = *v + 1;
    }
    t
}

pub fn locked(m: &Mutex<Counter>) -> u64 {
    let g = m.lock().unwrap();
    let c = g.count;
    let name_len = g.name.len();
    c + name_len as u64
}

pub fn shapes_demo() -> f64 {
    let s = Shape::Circle(2.0);
    let r = Shape::Rect { w: 1.0, h: 3.0 };
    let p = Pt::new(1, 2);
    let q: Pt = Pt { x: p.x, ..ORIGIN };
    let all = vec![s, r];
    let mut sum = 0.0;
    for sh in &all {
        sum += area(sh);
    }
    let d = p.dist2(&q);
    let first = &all[0];
    sum + d as f64 + area(first) + util::scale(2.0) + LIMIT as f64
}

pub fn closures(xs: Vec<i64>) -> i64 {
    let add = |a: i64, b: i64| a + b;
    let mut acc = 0;
    for x in xs.iter() {
        acc = add(acc, *x);
    }
    let double = move |v: i64| v * 2;
    let h = Hidden { v: 3 };
    double(acc) + h.get() as i64 + inner::deep() as i64
}

pub fn matching(e: Error) -> i32 {
    match e {
        Error::Empty => 0,
        Error::Bad(msg) => msg.len() as i32,
        Error::Code { n } => n,
    }
}

pub fn shadow(x: i32) -> i32 {
    let x = x + 1;
    let (a, b) = (x, x * 2);
    let y = {
        let x = a * b;
        x - 1
    };
    if let Some(z) = Some(y) {
        return z + x;
    }
    while let Some(w) = None::<i32> {
        let _ = w;
    }
    x
}

#[cfg(feature = "std")]
pub fn with_std() -> Counter {
    let mut c = Counter::new("std");
    c.bump().bump();
    c
}

#[cfg(not(feature = "std"))]
pub fn with_std() -> Counter {
    Counter::new("nostd")
}

#[cfg(all(feature = "std", feature = "extra"))]
pub fn extra_only() -> u8 {
    1
}

#[cfg(any(feature = "extra", test))]
pub fn extra_or_test() -> u8 {
    2
}

#[cfg(unix)]
pub fn platform() -> &'static str {
    "unix"
}

#[cfg(windows)]
pub fn platform() -> &'static str {
    "windows"
}

#[cfg(target_os = "linux")]
pub fn os_name() -> &'static str {
    "linux"
}

#[cfg(miri)]
pub fn under_miri() {}

macro_rules! square {
    ($e:expr) => {
        $e * $e
    };
}

pub fn use_macro(n: i64) -> i64 {
    let s = square!(n);
    s + util::text::shout_len("hi") as i64
}

pub fn texts() -> usize {
    let w = Word::new("abc");
    let n = w.text.len() + w.len();
    n + shout_len("x")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_works() {
        let mut c = Counter::new("t");
        c.bump();
        assert_eq!(c.count, 1);
        assert_eq!(extra_or_test(), 2);
    }
}

impl From<i32> for Hidden {
    fn from(v: i32) -> Self {
        Hidden { v }
    }
}

pub fn convert(n: i32) -> i32 {
    let h: Hidden = n.into();
    let g = Hidden::from(n);
    let s = "abc".to_string();
    h.v + g.get() + s.len() as i32
}
