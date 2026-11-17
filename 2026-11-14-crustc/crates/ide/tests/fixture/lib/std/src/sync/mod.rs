pub use self::poison::{LockResult, Mutex, MutexGuard};

pub mod nonpoison;
mod poison;
