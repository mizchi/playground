pub struct Mutex {
    futex: u32,
}

impl Mutex {
    pub const fn new() -> Mutex {
        loop {}
    }
    pub fn lock(&self) {}
}
