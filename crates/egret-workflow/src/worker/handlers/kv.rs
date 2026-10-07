//! `io.kestra.plugin.core.kv.{Set, Get, GetKeys, Delete, Put, PurgeKV}` --
//! the core KV tasks W9c ports (W9c spec §8).
//!
//! W9c Task 4 adds only the six type names, which publish's `secret()` scan
//! (`spec::validate`'s `secret_findings`, spec §7) reads to find a `Set` or
//! `Put` whose `value` calls `secret()`. Task 5 adds the handlers.

/// `Set` (spec §8.1).
pub const SET: &str = "io.kestra.plugin.core.kv.Set";
/// `Get` (spec §8.2).
pub const GET: &str = "io.kestra.plugin.core.kv.Get";
/// `GetKeys` (spec §8.3).
pub const GET_KEYS: &str = "io.kestra.plugin.core.kv.GetKeys";
/// `Delete` (spec §8.4).
pub const DELETE: &str = "io.kestra.plugin.core.kv.Delete";
/// `Put` (spec §8.5).
pub const PUT: &str = "io.kestra.plugin.core.kv.Put";
/// `PurgeKV` (spec §8.6).
pub const PURGE_KV: &str = "io.kestra.plugin.core.kv.PurgeKV";
