use std::sync::LazyLock;

use redis::Script;

/// Atomically deletes KEYS[1] only if its value matches ARGV[1].
/// Returns 1 when deleted, or 0 when absent or owned by another connection.
pub static COMPARE_AND_DELETE: LazyLock<Script> = LazyLock::new(|| Script::new(include_str!("compare_and_delete.lua")));

/// Atomically refreshes KEYS[1] with ARGV[2] seconds only when its value matches ARGV[1].
/// Returns 1 when refreshed, or 0 when absent or owned by another connection.
pub static COMPARE_AND_EXPIRE: LazyLock<Script> = LazyLock::new(|| Script::new(include_str!("compare_and_expire.lua")));

/// Atomically refreshes KEYS[1] with ARGV[2] seconds when its value matches ARGV[1], or sets it when absent.
/// Returns 1 when refreshed or claimed, or 0 when owned by another connection.
pub static COMPARE_AND_EXPIRE_OR_CLAIM: LazyLock<Script> =
    LazyLock::new(|| Script::new(include_str!("compare_and_expire_or_claim.lua")));
