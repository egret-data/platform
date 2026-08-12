//! Is this webhook URL safe to send to?
//!
//! A webhook destination is attacker-chosen by construction: any workspace
//! editor can type one. Without this check, "POST some JSON to a URL" is a
//! request forgery primitive that runs inside the cluster — it can reach the
//! cloud metadata endpoint, an unauthenticated in-cluster admin API, or
//! anything else that trusts the network it sits on.
//!
//! Two rules make the difference between a check and the appearance of one:
//!
//! 1. **Decide on the RESOLVED ADDRESS, not on the text.** A hostname is not a
//!    destination; the IP it resolves to is. `internal.attacker.com` is a
//!    perfectly ordinary public name that can resolve to `10.0.0.5`, and no
//!    amount of string inspection sees that.
//! 2. **Parse the URL, never scan it.** This repo has already shipped a check
//!    that read a path as a host (see the `url-normaliser` note in the design).
//!    The dangerous shape is `https://trusted.example@169.254.169.254/` — the
//!    host is the metadata address, and anything that looks for the first
//!    recognisable name finds `trusted.example` instead.
//!
//! The resolver is injected so both rules are testable without a network: the
//! interesting cases are exactly the ones a real DNS lookup will not reproduce
//! on demand.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::Url;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DestinationError {
    #[error("webhook url is not a valid url")]
    BadUrl,
    #[error("webhook url must be http or https")]
    BadScheme,
    #[error("webhook url has no host")]
    NoHost,
    #[error("webhook host does not resolve")]
    Unresolvable,
    #[error("webhook host resolves to a blocked address ({0}): {1}")]
    Blocked(IpAddr, &'static str),
}

/// DNS, behind a trait so the guard's decisions are testable.
#[async_trait::async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, std::io::Error>;
}

/// The real one.
pub struct SystemResolver;

#[async_trait::async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, std::io::Error> {
        Ok(tokio::net::lookup_host((host, port)).await?.collect())
    }
}

/// Operator escape hatch: hosts that may resolve into otherwise-blocked space.
///
/// Exists because "webhook to our own in-cluster incident bot" is a real and
/// reasonable request, and without a supported way to allow it someone will
/// instead turn the whole check off. Matched on the URL's host, exactly, before
/// resolution — an operator naming a host is vouching for that name.
#[derive(Debug, Clone, Default)]
pub struct SsrfPolicy {
    pub allow_hosts: Vec<String>,
}

impl SsrfPolicy {
    pub fn allows(&self, host: &str) -> bool {
        self.allow_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host))
    }
}

/// Vet `url` and return the addresses it is allowed to be sent to.
///
/// Returning the vetted addresses is not a convenience: the caller pins them on
/// the HTTP client, so the request goes to an address this function approved
/// rather than to whatever a second DNS lookup returns a moment later. Checking
/// and then letting the client re-resolve is a rebinding hole.
pub async fn vet(
    url: &str,
    policy: &SsrfPolicy,
    resolver: &dyn Resolver,
) -> Result<Vec<SocketAddr>, DestinationError> {
    let parsed = Url::parse(url).map_err(|_| DestinationError::BadUrl)?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(DestinationError::BadScheme),
    }
    // `Url::host_str` is the parsed authority's host — userinfo, path and query
    // cannot reach it. This is the whole defence against the
    // `https://trusted.example@169.254.169.254/` shape.
    let host = parsed.host_str().ok_or(DestinationError::NoHost)?;
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });

    let addrs = resolver
        .resolve(host, port)
        .await
        .map_err(|_| DestinationError::Unresolvable)?;
    if addrs.is_empty() {
        return Err(DestinationError::Unresolvable);
    }

    if policy.allows(host) {
        return Ok(addrs);
    }

    // EVERY address must be acceptable, not merely one. A name that resolves to
    // both a public and a private address would otherwise be usable to reach
    // the private one, since which address the client picks is not ours to
    // decide.
    for addr in &addrs {
        if let Some(reason) = blocked_reason(addr.ip()) {
            return Err(DestinationError::Blocked(addr.ip(), reason));
        }
    }
    Ok(addrs)
}

/// Why this address must not be dialled, or `None` if it is ordinary public
/// space.
///
/// Written as an explicit list rather than leaning on `std`'s helpers because
/// several of the ranges that matter most here — carrier-grade NAT, the
/// IPv4-mapped IPv6 form — are not covered by stable `is_*` methods, and a gap
/// in this list is invisible until it is exploited.
pub fn blocked_reason(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (`::ffff:10.0.0.1`) is an IPv4 destination
            // wearing an IPv6 shape; judging it as IPv6 would let every private
            // v4 range through.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return blocked_v4(mapped);
            }
            blocked_v6(v6)
        }
    }
}

fn blocked_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    if ip.is_unspecified() {
        return Some("unspecified");
    }
    if ip.is_loopback() {
        return Some("loopback");
    }
    if ip.is_private() {
        return Some("private");
    }
    if ip.is_link_local() {
        // Covers 169.254.169.254, the cloud metadata address that makes this
        // whole check load-bearing.
        return Some("link-local");
    }
    if ip.is_broadcast() {
        return Some("broadcast");
    }
    if ip.is_multicast() {
        return Some("multicast");
    }
    if ip.is_documentation() {
        return Some("documentation");
    }
    if a == 100 && (64..128).contains(&b) {
        return Some("carrier-grade NAT");
    }
    if a == 192 && b == 0 && ip.octets()[2] == 0 {
        return Some("IETF protocol assignment");
    }
    if a == 198 && (18..20).contains(&b) {
        return Some("benchmarking");
    }
    if a >= 240 {
        return Some("reserved");
    }
    None
}

fn blocked_v6(ip: Ipv6Addr) -> Option<&'static str> {
    if ip.is_unspecified() {
        return Some("unspecified");
    }
    if ip.is_loopback() {
        return Some("loopback");
    }
    if ip.is_multicast() {
        return Some("multicast");
    }
    let first = ip.segments()[0];
    if (first & 0xfe00) == 0xfc00 {
        return Some("unique local");
    }
    if (first & 0xffc0) == 0xfe80 {
        return Some("link-local");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A resolver with a fixed table, so "this public name resolves into
    /// private space" is a test rather than a hope.
    #[derive(Default)]
    struct FakeDns(HashMap<String, Vec<IpAddr>>);

    impl FakeDns {
        fn with(mut self, host: &str, ips: &[&str]) -> Self {
            self.0.insert(
                host.to_string(),
                ips.iter().map(|s| s.parse().unwrap()).collect(),
            );
            self
        }
    }

    #[async_trait::async_trait]
    impl Resolver for FakeDns {
        async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, std::io::Error> {
            match self.0.get(host) {
                Some(ips) => Ok(ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect()),
                None => Err(std::io::Error::other("NXDOMAIN")),
            }
        }
    }

    fn dns() -> FakeDns {
        FakeDns::default()
            .with("hooks.example.com", &["93.184.216.34"])
            .with("metadata.attacker.test", &["169.254.169.254"])
            .with("internal.attacker.test", &["10.0.0.5"])
            .with("split.attacker.test", &["93.184.216.34", "10.0.0.5"])
            .with("v6.attacker.test", &["::ffff:10.0.0.5"])
            .with("bot.internal", &["10.1.2.3"])
            .with("169.254.169.254", &["169.254.169.254"])
            .with("127.0.0.1", &["127.0.0.1"])
            .with("10.0.0.1", &["10.0.0.1"])
            .with("[::1]", &["::1"])
    }

    async fn vet_it(url: &str) -> Result<Vec<SocketAddr>, DestinationError> {
        vet(url, &SsrfPolicy::default(), &dns()).await
    }

    #[tokio::test]
    async fn an_ordinary_public_webhook_is_allowed() {
        // The other half of every rejection below: a guard that blocks
        // everything is not a guard, it is an outage.
        let ok = vet_it("https://hooks.example.com/services/T123")
            .await
            .unwrap();
        assert_eq!(ok[0].ip().to_string(), "93.184.216.34");
        assert_eq!(ok[0].port(), 443, "https defaults to 443");
    }

    #[tokio::test]
    async fn the_cloud_metadata_address_is_blocked() {
        // The single most valuable target in a cluster: unauthenticated, and
        // hands out credentials to anything that can make an HTTP request.
        let err = vet_it("http://169.254.169.254/latest/meta-data/")
            .await
            .unwrap_err();
        assert!(
            matches!(err, DestinationError::Blocked(_, "link-local")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn loopback_and_private_literals_are_blocked() {
        for url in [
            "http://127.0.0.1:8080/hook",
            "http://10.0.0.1/hook",
            "http://[::1]:9000/hook",
        ] {
            assert!(
                matches!(vet_it(url).await, Err(DestinationError::Blocked(..))),
                "{url} must be blocked"
            );
        }
    }

    #[tokio::test]
    async fn a_public_name_that_resolves_into_private_space_is_blocked() {
        // THE case that makes post-resolution checking non-negotiable: nothing
        // about the string `internal.attacker.test` is suspicious.
        let err = vet_it("https://internal.attacker.test/hook")
            .await
            .unwrap_err();
        assert!(
            matches!(err, DestinationError::Blocked(_, "private")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_name_resolving_to_both_public_and_private_is_blocked() {
        // Which address the client dials is not ours to choose, so one bad
        // answer poisons the name.
        let err = vet_it("https://split.attacker.test/hook")
            .await
            .unwrap_err();
        assert!(
            matches!(err, DestinationError::Blocked(_, "private")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn an_ipv4_mapped_ipv6_address_is_judged_as_ipv4() {
        // `::ffff:10.0.0.5` is a private IPv4 destination in an IPv6 coat.
        let err = vet_it("https://v6.attacker.test/hook").await.unwrap_err();
        assert!(
            matches!(err, DestinationError::Blocked(_, "private")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn userinfo_cannot_disguise_the_real_host() {
        // The recorded trap, in its dangerous direction: a check that looks for
        // the first plausible hostname finds `hooks.example.com` and waves
        // through a request to the metadata service.
        let err = vet(
            "https://hooks.example.com@169.254.169.254/latest/",
            &SsrfPolicy::default(),
            &dns(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, DestinationError::Blocked(_, "link-local")),
            "the authority's host is 169.254.169.254, not the userinfo: {err:?}"
        );
    }

    #[tokio::test]
    async fn a_private_address_in_the_path_does_not_condemn_a_public_host() {
        // The same trap in its false-positive direction. `https://evil.test/http://127.0.0.1/`
        // sends to evil.test, which is an ordinary (if unwise) public webhook —
        // blocking it would be substring-matching, not address checking, and a
        // check that fires on the wrong signal is one nobody can reason about.
        let ok = vet_it("https://hooks.example.com/http://127.0.0.1/x").await;
        assert!(ok.is_ok(), "decision must be made on the host: {ok:?}");
    }

    #[tokio::test]
    async fn non_http_schemes_are_refused() {
        for url in ["file:///etc/passwd", "gopher://evil/", "ftp://internal/x"] {
            assert_eq!(vet_it(url).await, Err(DestinationError::BadScheme), "{url}");
        }
    }

    #[tokio::test]
    async fn a_host_that_does_not_resolve_is_refused_rather_than_dialled() {
        assert_eq!(
            vet_it("https://nothing-here.test/hook").await,
            Err(DestinationError::Unresolvable)
        );
    }

    #[tokio::test]
    async fn the_allow_list_admits_a_named_internal_host_and_only_that_host() {
        let policy = SsrfPolicy {
            allow_hosts: vec!["bot.internal".into()],
        };
        let ok = vet("http://bot.internal/incident", &policy, &dns()).await;
        assert!(
            ok.is_ok(),
            "an operator-vouched host must be reachable: {ok:?}"
        );

        // The allow-list is a host list, not a switch: everything else stays
        // blocked, including another private address.
        let err = vet("http://10.0.0.1/hook", &policy, &dns())
            .await
            .unwrap_err();
        assert!(matches!(err, DestinationError::Blocked(..)), "{err:?}");
    }

    #[test]
    fn the_blocked_range_table_covers_what_it_claims() {
        // Spot-checks per family, so a range deleted from the list fails here
        // rather than in production.
        let cases: &[(&str, Option<&str>)] = &[
            ("0.0.0.0", Some("unspecified")),
            ("127.0.0.1", Some("loopback")),
            ("10.255.255.255", Some("private")),
            ("172.16.0.1", Some("private")),
            ("172.31.255.255", Some("private")),
            ("192.168.1.1", Some("private")),
            ("169.254.169.254", Some("link-local")),
            ("100.64.0.1", Some("carrier-grade NAT")),
            ("100.127.255.255", Some("carrier-grade NAT")),
            ("224.0.0.1", Some("multicast")),
            ("255.255.255.255", Some("broadcast")),
            ("240.0.0.1", Some("reserved")),
            ("::1", Some("loopback")),
            ("fd00::1", Some("unique local")),
            ("fe80::1", Some("link-local")),
            // Ordinary public space, both families.
            ("93.184.216.34", None),
            ("8.8.8.8", None),
            ("172.32.0.1", None),
            ("100.128.0.1", None),
            ("2606:4700::1111", None),
        ];
        for (ip, expected) in cases {
            assert_eq!(
                blocked_reason(ip.parse().unwrap()),
                *expected,
                "{ip} classified wrongly"
            );
        }
    }
}
