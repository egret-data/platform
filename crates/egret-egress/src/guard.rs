//! Is this URL safe to send to?
//!
//! Moved here from `egret-automation/src/deliver/destination.rs` so that alert
//! webhooks and the workflow `http.*` tasks share one guard.
//!
//! A webhook destination or an `http.Request` URI is attacker-chosen by
//! construction: any workspace editor can type one. Without this check, "send
//! a request to a URL" is a request forgery primitive that runs inside the
//! cluster — it can reach the cloud metadata endpoint, an unauthenticated
//! in-cluster admin API, or anything else that trusts the network it sits on.
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

use url::{Host, Url};

/// Why a URL was refused.
///
/// The texts carry no feature name: a caller prefixes its own
/// (`egret-automation` renders them as `webhook {0}`, which reproduces the
/// texts this guard had before it moved).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DestinationError {
    #[error("url is not a valid url")]
    BadUrl,
    #[error("url must be http or https")]
    BadScheme,
    #[error("url has no host")]
    NoHost,
    #[error("host does not resolve")]
    Unresolvable,
    #[error("host resolves to a blocked address ({0}): {1}")]
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
/// resolution — an operator naming a host is vouching for that name, whatever
/// it resolves to (a listed name that later resolves to the metadata address
/// is still allowed; the list is trusted, not re-judged).
#[derive(Debug, Clone, Default)]
pub struct SsrfPolicy {
    pub allow_hosts: Vec<String>,
}

impl SsrfPolicy {
    /// `host` is a URL's `host_str`. A name matches case-insensitively; an IP
    /// literal matches by address, so a bare IPv6 entry (`fd00::1`) matches
    /// the bracketed host the URL gives (`[fd00::1]`), and so does any other
    /// spelling of the same address (final review M1).
    pub fn allows(&self, host: &str) -> bool {
        let ip = literal_ip(host);
        self.allow_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host) || (ip.is_some() && literal_ip(h) == ip))
    }
}

/// `s` as an IP address, with or without IPv6 brackets.
fn literal_ip(s: &str) -> Option<IpAddr> {
    s.strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .unwrap_or(s)
        .parse()
        .ok()
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
    vet_url(&parsed, policy, resolver).await
}

/// [`vet`] for an already-parsed URL (the redirect follower has one).
pub async fn vet_url(
    parsed: &Url,
    policy: &SsrfPolicy,
    resolver: &dyn Resolver,
) -> Result<Vec<SocketAddr>, DestinationError> {
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

    // An IP literal is its own address: no lookup. (`host_str` of an IPv6
    // literal is `[::1]`, which no resolver understands; the WHATWG parser has
    // already normalised forms like `0x7f.1` or `2130706433` to `127.0.0.1`.)
    let addrs = match parsed.host() {
        Some(Host::Ipv4(v4)) => vec![SocketAddr::new(IpAddr::V4(v4), port)],
        Some(Host::Ipv6(v6)) => vec![SocketAddr::new(IpAddr::V6(v6), port)],
        _ => resolver
            .resolve(host, port)
            .await
            .map_err(|_| DestinationError::Unresolvable)?,
    };
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
/// IPv4-mapped IPv6 form, the IPv6 transition prefixes — are not covered by
/// stable `is_*` methods, and a gap in this list is invisible until it is
/// exploited. The source for both tables is the IANA special-purpose address
/// registries (RFC 6890 and its updates); each entry names its RFC.
pub fn blocked_reason(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (`::ffff:10.0.0.1`, RFC 4291 §2.5.5.2) is
            // an IPv4 destination wearing an IPv6 shape; judging it as IPv6
            // would let every private v4 range through.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return blocked_v4(mapped);
            }
            blocked_v6(v6)
        }
    }
}

fn blocked_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, c, _] = ip.octets();
    if ip.is_unspecified() {
        return Some("unspecified");
    }
    if a == 0 {
        // 0.0.0.0/8 "this network" (RFC 791, RFC 1122 §3.2.1.3). Not only
        // 0.0.0.0: Linux dials 0.x.y.z as a local address.
        return Some("this network");
    }
    if ip.is_loopback() {
        // 127.0.0.0/8 (RFC 1122).
        return Some("loopback");
    }
    if ip.is_private() {
        // 10/8, 172.16/12, 192.168/16 (RFC 1918).
        return Some("private");
    }
    if ip.is_link_local() {
        // 169.254.0.0/16 (RFC 3927). Covers 169.254.169.254, the cloud
        // metadata address that makes this whole check load-bearing.
        return Some("link-local");
    }
    if ip.is_broadcast() {
        // 255.255.255.255 (RFC 919).
        return Some("broadcast");
    }
    if ip.is_multicast() {
        // 224.0.0.0/4 (RFC 5771).
        return Some("multicast");
    }
    if ip.is_documentation() {
        // 192.0.2/24, 198.51.100/24, 203.0.113/24 (RFC 5737).
        return Some("documentation");
    }
    if a == 100 && (64..128).contains(&b) {
        // 100.64.0.0/10 (RFC 6598).
        return Some("carrier-grade NAT");
    }
    if a == 192 && b == 0 && c == 0 {
        // 192.0.0.0/24 (RFC 6890), incl. DS-Lite 192.0.0.0/29 (RFC 7335)
        // and the NAT64 discovery addresses 192.0.0.170/171 (RFC 7050).
        return Some("IETF protocol assignment");
    }
    if a == 192 && b == 88 && c == 99 {
        // 192.88.99.0/24, the 6to4 relay anycast (RFC 3068, deprecated by
        // RFC 7526): a relay that would forward into an embedded address.
        return Some("6to4 relay anycast");
    }
    if a == 198 && (18..20).contains(&b) {
        // 198.18.0.0/15 (RFC 2544).
        return Some("benchmarking");
    }
    if a >= 240 {
        // 240.0.0.0/4 (RFC 1112).
        return Some("reserved");
    }
    None
}

fn blocked_v6(ip: Ipv6Addr) -> Option<&'static str> {
    let s = ip.segments();
    if ip.is_unspecified() {
        // ::/128 (RFC 4291).
        return Some("unspecified");
    }
    if ip.is_loopback() {
        // ::1/128 (RFC 4291).
        return Some("loopback");
    }
    if ip.is_multicast() {
        // ff00::/8 (RFC 4291).
        return Some("multicast");
    }
    if s[..6] == [0, 0, 0, 0, 0, 0] {
        // ::/96, the IPv4-compatible form `::a.b.c.d` (RFC 4291 §2.5.5.1,
        // deprecated). Refused outright: it has no legitimate use, and a stack
        // that still honours it sends to the embedded v4 address.
        return Some("IPv4-compatible");
    }
    if s[..6] == [0, 0, 0, 0, 0xffff, 0] {
        // ::ffff:0:0:0/96, the IPv4-translated form (RFC 2765). Not routable.
        return Some("IPv4-translated");
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        // 64:ff9b::/96, the NAT64 well-known prefix (RFC 6052). Refused
        // whatever the embedded v4 address is: behind a NAT64 gateway even an
        // embedded PUBLIC address is reached from the gateway's position, and
        // an embedded private one is the gateway's own network. The safer
        // default is not to judge the payload but to refuse the prefix.
        return Some("NAT64");
    }
    if s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1 {
        // 64:ff9b:1::/48, local-use NAT64 (RFC 8215). Same reasoning.
        return Some("NAT64");
    }
    if s[..4] == [0x100, 0, 0, 0] {
        // 100::/64 discard-only (RFC 6666).
        return Some("discard-only");
    }
    if s[0] == 0x2001 && s[1] == 0 {
        // 2001::/32 Teredo (RFC 4380): the client's v4 address is embedded
        // (obfuscated), and the relay decides where the packet goes.
        return Some("Teredo");
    }
    if s[0] == 0x2001 && s[1] == 0xdb8 {
        // 2001:db8::/32 (RFC 3849).
        return Some("documentation");
    }
    if s[0] == 0x2001 && s[1] < 0x200 {
        // 2001::/23 IETF protocol assignments (RFC 2928, RFC 6890): ORCHID,
        // benchmarking 2001:2::/48 (RFC 5180), etc. Nothing an outbound
        // request needs; mirrors 192.0.0.0/24 on the v4 side.
        return Some("IETF protocol assignment");
    }
    if s[0] == 0x2002 {
        // 2002::/16 6to4 (RFC 3056): the destination is the embedded v4
        // address, reached through a relay. Refused outright, as NAT64.
        return Some("6to4");
    }
    if (s[0] & 0xfff0) == 0x3ff0 {
        // 3fff::/20 (RFC 9637).
        return Some("documentation");
    }
    if s[0] == 0x5f00 {
        // 5f00::/16 SRv6 SIDs (RFC 9602): not globally reachable.
        return Some("SRv6 SID");
    }
    if (s[0] & 0xfe00) == 0xfc00 {
        // fc00::/7 (RFC 4193). Covers the AWS IPv6 metadata fd00:ec2::254.
        return Some("unique local");
    }
    if (s[0] & 0xffc0) == 0xfe80 {
        // fe80::/10 (RFC 4291).
        return Some("link-local");
    }
    if (s[0] & 0xffc0) == 0xfec0 {
        // fec0::/10 site-local (RFC 3879, deprecated but still routed by
        // some stacks as private space).
        return Some("site-local");
    }
    if (s[0] & 0xe000) != 0x2000 {
        // Everything outside 2000::/3 is not global unicast (RFC 4291 §2.4,
        // IANA IPv6 address space registry). Refusing it closes ranges this
        // table does not name yet rather than letting them through.
        return Some("not global unicast");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Final review M1: a bare IPv6 entry matches the bracketed host a URL
    /// gives, by address, and a different address does not.
    #[test]
    fn an_ipv6_allowlist_entry_matches_with_or_without_brackets() {
        let url = Url::parse("http://[fd00::1]:8080/").unwrap();
        let host = url.host_str().unwrap();
        assert_eq!(host, "[fd00::1]");
        for entry in ["fd00::1", "[fd00::1]", "FD00:0:0::1"] {
            let p = SsrfPolicy {
                allow_hosts: vec![entry.into()],
            };
            assert!(p.allows(host), "{entry}");
        }
        let other = SsrfPolicy {
            allow_hosts: vec!["fd00::2".into(), "a.test".into()],
        };
        assert!(!other.allows(host));
        assert!(other.allows("A.TEST"));
        assert!(!other.allows("b.test"));
    }

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
    async fn ip_literals_are_judged_without_asking_the_resolver() {
        // `FakeDns::default()` knows no names, so any lookup would answer
        // Unresolvable; a literal must reach the range table instead. The
        // obfuscated v4 spellings are normalised by the URL parser.
        let empty = FakeDns::default();
        for (url, reason) in [
            ("http://127.0.0.1/", "loopback"),
            ("http://2130706433/", "loopback"),
            ("http://0x7f.1/", "loopback"),
            ("http://0.0.0.5/", "this network"),
            ("http://[::1]/", "loopback"),
            ("http://[::ffff:169.254.169.254]/", "link-local"),
            ("http://[64:ff9b::a9fe:a9fe]/", "NAT64"),
        ] {
            let err = vet(url, &SsrfPolicy::default(), &empty).await.unwrap_err();
            assert_eq!(
                err,
                DestinationError::Blocked(
                    match Url::parse(url).unwrap().host().unwrap() {
                        Host::Ipv4(v4) => IpAddr::V4(v4),
                        Host::Ipv6(v6) => IpAddr::V6(v6),
                        Host::Domain(d) => panic!("{url} parsed as domain {d}"),
                    },
                    reason
                ),
                "{url}"
            );
        }
        let ok = vet("https://8.8.8.8/x", &SsrfPolicy::default(), &empty).await;
        assert_eq!(ok.unwrap()[0].to_string(), "8.8.8.8:443");
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
            // C1 additions, one row per range and its edges.
            // 0.0.0.0/8, beyond 0.0.0.0 itself.
            ("0.0.0.1", Some("this network")),
            ("0.255.255.255", Some("this network")),
            ("1.0.0.0", None),
            // 192.88.99.0/24 6to4 relay anycast.
            ("192.88.99.1", Some("6to4 relay anycast")),
            ("192.88.98.255", None),
            ("192.0.0.170", Some("IETF protocol assignment")),
            ("198.18.0.1", Some("benchmarking")),
            ("192.0.2.1", Some("documentation")),
            // 64:ff9b::/96 NAT64, with a private AND a public embedded v4:
            // the prefix is refused, not the payload judged.
            ("64:ff9b::10.0.0.5", Some("NAT64")),
            ("64:ff9b::8.8.8.8", Some("NAT64")),
            ("64:ff9b:1::1", Some("NAT64")),
            // 2002::/16 6to4, private and public embedded v4.
            ("2002:a00:5::1", Some("6to4")),
            ("2002:808:808::1", Some("6to4")),
            // 2001::/32 Teredo.
            ("2001::1", Some("Teredo")),
            ("2001:0:ffff:ffff::1", Some("Teredo")),
            ("2001:2::1", Some("IETF protocol assignment")),
            ("2001:db8::1", Some("documentation")),
            ("3fff::1", Some("documentation")),
            // ::a.b.c.d, the IPv4-compatible form, private and public payload.
            ("::10.0.0.5", Some("IPv4-compatible")),
            ("::8.8.8.8", Some("IPv4-compatible")),
            ("::ffff:0:10.0.0.5", Some("IPv4-translated")),
            // v4-mapped is judged as v4, both ways.
            ("::ffff:169.254.169.254", Some("link-local")),
            ("::ffff:8.8.8.8", None),
            ("100::1", Some("discard-only")),
            ("5f00::1", Some("SRv6 SID")),
            ("fec0::1", Some("site-local")),
            // AWS IPv6 metadata.
            ("fd00:ec2::254", Some("unique local")),
            ("ff02::1", Some("multicast")),
            // Outside 2000::/3.
            ("4000::1", Some("not global unicast")),
            ("1::1", Some("not global unicast")),
            // Ordinary public space, both families.
            ("93.184.216.34", None),
            ("8.8.8.8", None),
            ("172.32.0.1", None),
            ("100.128.0.1", None),
            ("2606:4700::1111", None),
            ("2001:200::1", None),
            ("2001:4860:4860::8888", None),
            ("2003::1", None),
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
