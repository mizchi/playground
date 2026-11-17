/// A UTF-8–encoded, growable string.
pub struct String {
    vec: Vec<u8>,
}

impl String {
    pub const fn new() -> String {
        loop {}
    }
    pub fn push_str(&mut self, string: &str) {
        loop {}
    }
    pub fn as_str(&self) -> &str {
        loop {}
    }
}

impl Deref for String {
    type Target = str;
    fn deref(&self) -> &str {
        loop {}
    }
}

impl Clone for String {
    fn clone(&self) -> Self {
        loop {}
    }
}

pub trait ToString {
    fn to_string(&self) -> String;
}

impl<T: Display + ?Sized> ToString for T {
    fn to_string(&self) -> String {
        loop {}
    }
}
