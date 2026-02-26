use std::time::Duration;

/// Maximum time to wait when acquiring the store's read or write lock.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a tombstone entry lives before the TTL mechanism may expire it (seconds).
pub const TOMBSTONE_TTL_SECS: u64 = 3600;
