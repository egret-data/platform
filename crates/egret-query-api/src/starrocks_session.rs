//! The two session settings egret's MySQL-protocol connections to a StarRocks
//! FE state up front, shared by egret-engine-mgmt's root connection
//! (`starrocks_root_conn_opts`) and egret-query's `StarRocksConnector`.
//!
//! `mysql_async` (0.37.0, vendored in `third_party/mysql_async`) runs `SELECT
//! @@max_allowed_packet,@@wait_timeout` on every new connection unless both
//! are set in its `Opts` (`Conn::read_settings`; `@@socket` is read only with
//! `prefer_socket`, which both connections turn off). On an engine with a
//! Query log that statement carries no read marker, so every connection put a
//! row in the user's log (live check, 2026-11-11). Setting both skips it.
//!
//! The values are StarRocks' own session defaults, so a connection behaves as
//! it did when the driver read them from the FE.

/// StarRocks' session default `max_allowed_packet`: 32 MiB.
pub const STARROCKS_MAX_ALLOWED_PACKET: usize = 32 * 1024 * 1024;

/// StarRocks' session default `wait_timeout`, in seconds: 8 hours.
pub const STARROCKS_WAIT_TIMEOUT_SECS: usize = 28_800;
