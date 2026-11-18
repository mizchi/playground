//! Item forms that the symbol scanner must handle.
#![allow(unused)]

#[doc = "attr"]
#[inline]
pub(crate) const fn cfn<const N: usize>() -> usize {
    N
}
pub const unsafe fn cufn() {}
pub unsafe extern "C" fn cabi(x: i32) -> i32 {
    x
}
extern "C" fn cabi2() {}
async fn later() {}
pub(in crate::odd) fn scoped() {}

extern "C" {
    fn ext_a(x: u8) -> u8;
    static EXT_B: u32;
}
extern {
    fn ext_c();
}
extern crate core as kore;
use std::fmt::{self, Debug as Dbg};

pub union U {
    pub a: u32,
    b: f32,
}

#[repr(C)]
pub struct Tuple(pub u8, u16);
pub struct Unit;
pub struct Gen<'a, T: 'a + Clone, const N: usize>
where
    T: Default,
{
    #[allow(unused)]
    pub(crate) r: &'a [T; N],
    f: fn(u8) -> (u8, u8),
    m: Vec<Vec<(u8, u16)>>,
    c: Option<Box<dyn Fn(i32) -> i32 + 'a>>,
}

pub enum E {
    #[allow(unused)]
    A = 1,
    B(u8, (u16, u32)),
    C { x: [u8; 4], y: Vec<i32> },
    D,
}

pub trait Tr<T>: Clone
where
    T: Copy,
{
    type Out;
    const K: usize = 3;
    fn req(&self) -> Self::Out;
    fn prov(&self) -> usize {
        Self::K
    }
}

unsafe impl Send for Unit {}
impl<'a, T: Clone + Default, const N: usize> Gen<'a, T, N> where T: Copy {
    pub fn r(&self) -> &'a [T; N] {
        self.r
    }
    const C: u8 = 1;
    type X = u8;
}
impl<T> Tr<T> for Unit
where
    T: Copy,
{
    type Out = ();
    fn req(&self) -> () {}
}
impl !Sync for Tuple {}
impl Clone for Unit {
    fn clone(&self) -> Self {
        Unit
    }
}
const _: () = ();
static mut COUNTER: u64 = 0;
pub type Alias<K, V> = std::collections::HashMap<K, Vec<V>>;
type Fnp = fn(&str) -> Result<(), String>;
type Short = u8;

macro_rules! mac {
    ($x:expr) => {
        $x
    };
}
macro_rules! paren_mac(() => {});
macro_rules! brack_mac[() => {}];
pub macro m2($x:expr) {
    $x
}
pub macro m3 {
    () => {}
}
mac!(1);
thread_local! {
    static TL: u8 = 0;
}
mod outer {
    pub mod mid {
        pub fn leaf<'a>(s: &'a str) -> &'a str {
            s
        }
    }
    fn sib() {}
}
mod external;
