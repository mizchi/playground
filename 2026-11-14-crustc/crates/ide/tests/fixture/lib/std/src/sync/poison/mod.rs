pub use self::mutex::{Mutex, MutexGuard};

mod mutex;

pub type LockResult<T> = Result<T, PoisonError<T>>;

pub struct PoisonError<T> {
    data: T,
}
