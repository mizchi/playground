pub struct Mutex<T: ?Sized> {
    data: UnsafeCell<T>,
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> MutexGuard<'_, T> {
        loop {}
    }
}
