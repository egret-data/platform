//! A connect-only reachability probe (D4). It opens a TCP connection, learns
//! whether it opened, and closes it: no bytes are sent, none are read, and no
//! protocol is spoken. It cannot tell you that credentials work — which is
//! exactly why the route reports WHICH check ran, and the UI prints it.
//!
//! The target always comes from `CatalogDef::probe_target`, never from a
//! request body or query. There is deliberately no private-range block list:
//! egret's own engines and metastores are in-cluster, so refusing RFC1918
//! would break the real use case while stopping nobody who already holds
//! `catalog_registry:create`.
//!
//! It lives here rather than in `egret-catalog-registry`, which owns the
//! configuration, because that crate carries `tokio` as a dev-dependency only:
//! the pure decision (`CatalogDef::probe_target`) stays with the configuration
//! and the I/O stays with the crate that already runs a reactor.

use std::time::Duration;

/// Short on purpose: this is a UI button, and an unreachable host must answer
/// the operator, not hold the request open for a TCP stack's own retry budget.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// What one connect attempt learned. Three outcomes, because "did not open"
/// and "never answered" are different things to an operator: the first is a
/// host that said no, the second a host, firewall or name that said nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The TCP handshake completed, and the connection was closed again.
    /// Something is listening. It is NOT known to be the configured catalog's
    /// metastore or database, and no credential was offered.
    Open,
    /// The connect failed outright — refused, no route, DNS failure. The
    /// string is the OS error, which names which of those it was.
    Refused(String),
    /// Nothing answered within [`PROBE_TIMEOUT`].
    TimedOut,
}

/// Dial `host:port`, then drop the socket. See the module docs for what this
/// does and does not prove.
pub async fn tcp_probe(host: &str, port: u16) -> ProbeOutcome {
    match tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect((host, port))).await {
        // Dropped right here: connect, then close. Nothing is written or read.
        Ok(Ok(_stream)) => ProbeOutcome::Open,
        Ok(Err(e)) => ProbeOutcome::Refused(e.to_string()),
        Err(_) => ProbeOutcome::TimedOut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_reports_open_for_a_listening_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // The listener stays alive for the whole probe: its backlog completes
        // the handshake even without an `accept`.
        assert_eq!(
            tcp_probe("127.0.0.1", addr.port()).await,
            ProbeOutcome::Open
        );
    }

    #[tokio::test]
    async fn probe_reports_refused_for_a_closed_port() {
        // Bind to learn a port the OS just handed out, then give it back, so
        // the port is almost certainly free — and never a port some unrelated
        // service on the developer's machine happens to hold.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        match tcp_probe("127.0.0.1", port).await {
            ProbeOutcome::Refused(e) => assert!(!e.is_empty(), "the OS error is surfaced"),
            other => panic!("a closed port must not read as {other:?}"),
        }
    }
}
