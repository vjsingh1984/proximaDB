// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-TENANT-4: resolve the client address from an **observation**, not an
//! assertion.
//!
//! Before this module, `get_client_ip` resolved `X-Forwarded-For` →
//! `X-Real-IP` → a hardcoded `127.0.0.1`, with the comment *"this would need to
//! be set by the server, for now use localhost"*. That comment named the root
//! cause: the real peer address was never available, because nothing wired
//! axum's [`ConnectInfo`](axum::extract::ConnectInfo). Lacking an observation,
//! the code substituted a client-controlled header and then a constant.
//!
//! The live consumer is **KOU egress metering** (the metrics middleware
//! classifies every REST request to decide chargeability), so the two
//! consequences were: any caller could opt out of metering by asserting a
//! private address, and — with no proxy in front — every request fell to
//! loopback and metering was inert by default.
//!
//! ## The rule
//!
//! 1. The transport peer address is the truth.
//! 2. A forwarded header overrides it **only** when the immediate peer is a
//!    declared trusted proxy. This is the [`HeaderTrustPolicy`]-shaped rule
//!    from TD-TENANT-1 — honor a bare assertion only from a caller entitled to
//!    make it — except the entitlement here is *network position*, so it is
//!    expressed as CIDRs rather than a credential.
//! 3. An untrusted forwarded header is **dropped, never rejected**, matching
//!    the drop-not-4xx semantics of ADR-0053 W8 and TD-TENANT-3.
//!
//! Default is an empty allowlist: trust no proxy, ignore the headers. That is
//! safe under direct exposure and correct for the default single-node
//! deployment; an operator fronting the engine with a load balancer declares
//! its CIDR via `PROXIMADB_TRUSTED_PROXY_CIDRS`.

use axum::extract::{ConnectInfo, Request};
use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::LazyLock;

/// TD-TENANT-4 follow-up: every `Unobserved` client-address resolution, on
/// any surface. Expected value: flat zero — all four TCP serve sites wire
/// `ConnectInfo` and the UDS path is marked `LocalSocket`. A rising counter
/// means a surface regressed to serving without
/// `into_make_service_with_connect_info`, silently degrading every
/// client-address decision on that surface (rate-limit keys, trusted-proxy
/// checks, KOU egress classification). Label-free by design: the seam does
/// not know its surface, and the response to any rise is "find the unwired
/// surface" — answerable from the serve-site list.
///
/// Registration failure degrades telemetry rather than panicking a request.
/// [`initialize_client_addr_metrics`] forces this lazy at metrics-service
/// startup so a healthy process exports an explicit zero instead of leaving
/// operators with absent-vs-zero ambiguity.
static UNOBSERVED_RESOLUTIONS: LazyLock<Option<prometheus::IntCounter>> =
    LazyLock::new(|| {
        match prometheus::register_int_counter!(
            "proximadb_client_addr_unobserved_total",
            "Client-address resolutions that found no transport peer and no \
             local-socket marker (a ConnectInfo wiring regression on some \
             surface). Expected flat zero; any rise is a bug."
        ) {
            Ok(counter) => Some(counter),
            Err(error) => {
                tracing::warn!(
                    target: "proximadb::tenant_audit",
                    %error,
                    "failed to register the unobserved client-address counter"
                );
                None
            }
        }
    });

/// Register client-address metrics before the first request is served.
pub fn initialize_client_addr_metrics() {
    LazyLock::force(&UNOBSERVED_RESOLUTIONS);
}

/// Env gate carrying the deployment's trusted reverse-proxy CIDRs, comma
/// separated (e.g. `10.0.0.0/8,192.168.1.5/32`). Unset or empty ⇒ trust no
/// proxy, so forwarded headers are ignored entirely.
pub const TRUSTED_PROXY_CIDRS_ENV: &str = "PROXIMADB_TRUSTED_PROXY_CIDRS";

/// Canonical loopback used when the peer is a Unix-domain socket. A UDS peer is
/// on the same host by construction, so this is the *correct* classification,
/// not a fallback.
const UDS_PEER: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// How the client address was determined — the provenance, kept so callers and
/// audits can tell an observation from an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAddrSource {
    /// Read from the transport peer address (`ConnectInfo<SocketAddr>`).
    Peer,
    /// Asserted by a peer that is a declared trusted proxy.
    TrustedProxy {
        /// Which forwarded header supplied the value.
        header: &'static str,
    },
    /// Unix-domain socket peer — same host by construction.
    LocalSocket,
    /// No peer address was available (`ConnectInfo` not wired on this surface).
    /// Kept distinct from `LocalSocket` so a wiring regression is visible
    /// instead of silently reading as a legitimate local client.
    Unobserved,
}

/// A resolved client address plus where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr {
    /// The address to classify / key on.
    pub ip: IpAddr,
    /// Its provenance.
    pub source: ClientAddrSource,
}

impl ClientAddr {
    /// Whether this address was actually observed rather than asserted by a
    /// client or defaulted. Callers that meter or bill should prefer observed
    /// addresses and can surface the difference.
    pub fn is_observed(&self) -> bool {
        !matches!(self.source, ClientAddrSource::Unobserved)
    }
}

/// The deployment's trusted reverse-proxy set. Empty ⇒ no proxy is trusted, so
/// forwarded headers are ignored.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    cidrs: Vec<IpNet>,
}

impl TrustedProxies {
    /// Parse a comma-separated CIDR list. Unparseable entries are skipped
    /// (fail-safe: a typo narrows trust, never widens it) and reported so a
    /// misconfiguration is visible rather than silently permissive.
    pub fn parse(raw: &str) -> (Self, Vec<String>) {
        let mut cidrs = Vec::new();
        let mut rejected = Vec::new();
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            // Accept a bare address as a host route, the common single-LB case.
            let parsed = entry
                .parse::<IpNet>()
                .ok()
                .or_else(|| entry.parse::<IpAddr>().ok().map(IpNet::from));
            match parsed {
                Some(net) => cidrs.push(net),
                None => rejected.push(entry.to_string()),
            }
        }
        (Self { cidrs }, rejected)
    }

    /// Read the allowlist from [`TRUSTED_PROXY_CIDRS_ENV`], warning once about
    /// any unparseable entry.
    pub fn from_env() -> Self {
        let Ok(raw) = std::env::var(TRUSTED_PROXY_CIDRS_ENV) else {
            return Self::default();
        };
        let (proxies, rejected) = Self::parse(&raw);
        if !rejected.is_empty() {
            tracing::warn!(
                target: "proximadb::tenant_audit",
                env = TRUSTED_PROXY_CIDRS_ENV,
                rejected = ?rejected,
                "ignoring unparseable trusted-proxy CIDR entries"
            );
        }
        proxies
    }

    /// Whether `peer` is a declared trusted proxy.
    pub fn trusts(&self, peer: IpAddr) -> bool {
        self.cidrs.iter().any(|net| net.contains(&peer))
    }

    /// Whether any proxy is trusted at all.
    pub fn is_empty(&self) -> bool {
        self.cidrs.is_empty()
    }
}

/// Resolve the client address for a request.
///
/// `peer` is the observed transport peer, if the surface supplies one. See the
/// module docs for the trust rule.
pub fn resolve_client_addr(
    peer: Option<IpAddr>,
    lookup_header: impl Fn(&str) -> Option<String>,
    trusted: &TrustedProxies,
) -> ClientAddr {
    let Some(peer) = peer else {
        // No observation. Do NOT consult forwarded headers here: without a peer
        // address there is no way to establish that a proxy is entitled to
        // assert one, so honoring the header would restore exactly the defect
        // this module closes.
        if let Some(counter) = UNOBSERVED_RESOLUTIONS.as_ref() {
            counter.inc();
        }
        return ClientAddr {
            ip: UDS_PEER,
            source: ClientAddrSource::Unobserved,
        };
    };

    if trusted.trusts(peer)
        && let Some((ip, header)) = forwarded_ip(&lookup_header)
    {
        return ClientAddr {
            ip,
            source: ClientAddrSource::TrustedProxy { header },
        };
    }

    ClientAddr {
        ip: peer,
        source: ClientAddrSource::Peer,
    }
}

/// Marker the UDS serve path inserts into request extensions
/// (`router.layer(Extension(LocalSocketPeer))`), since a Unix socket supplies no
/// `ConnectInfo<SocketAddr>`.
///
/// Without it, a UDS request would be indistinguishable from a TCP surface that
/// forgot to wire `ConnectInfo` — both would read `Unobserved`. Marking it keeps
/// [`ClientAddrSource::Unobserved`] meaning *"a wiring regression"*, which is
/// the only reason that variant is worth having.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSocketPeer;

/// Resolve for a Unix-domain-socket peer: same host by construction.
pub fn local_socket_client_addr() -> ClientAddr {
    ClientAddr {
        ip: UDS_PEER,
        source: ClientAddrSource::LocalSocket,
    }
}

/// First parseable forwarded address: leftmost `X-Forwarded-For` entry (the
/// original client in the standard left-to-right chain), else `X-Real-IP`.
fn forwarded_ip(lookup: &impl Fn(&str) -> Option<String>) -> Option<(IpAddr, &'static str)> {
    if let Some(xff) = lookup("x-forwarded-for")
        && let Some(ip) = xff
            .split(',')
            .next()
            .and_then(|first| first.trim().parse::<IpAddr>().ok())
    {
        return Some((ip, "x-forwarded-for"));
    }
    lookup("x-real-ip")
        .and_then(|v| v.trim().parse::<IpAddr>().ok())
        .map(|ip| (ip, "x-real-ip"))
}

/// Extract the client address from an axum request.
///
/// Reads `ConnectInfo<SocketAddr>` (wired at the TCP serve sites); a surface
/// that does not supply it — currently the UDS/portless path — yields
/// [`ClientAddrSource::Unobserved`].
pub fn client_addr_from_request(request: &Request, trusted: &TrustedProxies) -> ClientAddr {
    // A UDS peer is local by construction and carries no SocketAddr. Handle it
    // before the ConnectInfo lookup so it never reads as `Unobserved`.
    if request.extensions().get::<LocalSocketPeer>().is_some() {
        return local_socket_client_addr();
    }

    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let headers = request.headers();
    resolve_client_addr(
        peer,
        |name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(ToOwned::to_owned)
        },
        trusted,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn headers(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test ip")
    }

    fn trusting(cidrs: &str) -> TrustedProxies {
        let (t, rejected) = TrustedProxies::parse(cidrs);
        assert!(rejected.is_empty(), "test CIDRs must parse: {rejected:?}");
        t
    }

    /// D1: the defect. An untrusted caller asserting a private address must not
    /// be able to pick its own billing classification.
    #[test]
    fn untrusted_forwarded_header_is_dropped() {
        let resolved = resolve_client_addr(
            Some(ip("203.0.113.9")),
            headers(&[("x-forwarded-for", "10.0.0.1")]),
            &TrustedProxies::default(),
        );
        assert_eq!(
            resolved.ip,
            ip("203.0.113.9"),
            "must keep the observed peer"
        );
        assert_eq!(resolved.source, ClientAddrSource::Peer);
    }

    /// The same assertion IS honored from a declared proxy — otherwise a real
    /// load-balancer deployment would meter the balancer instead of the client.
    #[test]
    fn trusted_proxy_forwarded_header_is_honored() {
        let resolved = resolve_client_addr(
            Some(ip("10.0.0.7")),
            headers(&[("x-forwarded-for", "203.0.113.9")]),
            &trusting("10.0.0.0/8"),
        );
        assert_eq!(resolved.ip, ip("203.0.113.9"));
        assert_eq!(
            resolved.source,
            ClientAddrSource::TrustedProxy {
                header: "x-forwarded-for"
            }
        );
    }

    #[test]
    fn leftmost_forwarded_entry_wins_and_x_real_ip_is_the_fallback() {
        let chain = resolve_client_addr(
            Some(ip("10.0.0.7")),
            headers(&[("x-forwarded-for", "203.0.113.9, 70.41.3.18, 10.0.0.7")]),
            &trusting("10.0.0.0/8"),
        );
        assert_eq!(chain.ip, ip("203.0.113.9"));

        let real_ip = resolve_client_addr(
            Some(ip("10.0.0.7")),
            headers(&[("x-real-ip", "198.51.100.4")]),
            &trusting("10.0.0.0/8"),
        );
        assert_eq!(real_ip.ip, ip("198.51.100.4"));
        assert_eq!(
            real_ip.source,
            ClientAddrSource::TrustedProxy {
                header: "x-real-ip"
            }
        );
    }

    /// A trusted peer that sends garbage falls back to the observed address
    /// rather than to a constant.
    #[test]
    fn unparseable_forwarded_value_falls_back_to_the_peer() {
        let resolved = resolve_client_addr(
            Some(ip("10.0.0.7")),
            headers(&[("x-forwarded-for", "not-an-ip")]),
            &trusting("10.0.0.0/8"),
        );
        assert_eq!(resolved.ip, ip("10.0.0.7"));
        assert_eq!(resolved.source, ClientAddrSource::Peer);
    }

    /// D2: with no peer observation the address is marked `Unobserved` — and,
    /// critically, forwarded headers are NOT consulted, because there is no way
    /// to establish that anyone was entitled to assert one.
    #[test]
    fn unobserved_peer_does_not_fall_through_to_the_header() {
        let resolved = resolve_client_addr(
            None,
            headers(&[("x-forwarded-for", "203.0.113.9")]),
            &trusting("0.0.0.0/0"),
        );
        assert_eq!(resolved.source, ClientAddrSource::Unobserved);
        assert!(!resolved.is_observed());
        assert_ne!(resolved.ip, ip("203.0.113.9"));
    }

    #[test]
    fn uds_peer_is_local_not_unobserved() {
        let resolved = local_socket_client_addr();
        assert_eq!(resolved.source, ClientAddrSource::LocalSocket);
        assert!(
            resolved.is_observed(),
            "a UDS peer IS observed — same host by construction"
        );
    }

    /// The marker must actually be consulted by the request path — otherwise
    /// `LocalSocket` would be a variant nothing can ever produce, and a UDS
    /// request would masquerade as a `ConnectInfo` wiring regression.
    #[test]
    fn local_socket_marker_is_honored_by_the_request_path() {
        let mut request = Request::new(axum::body::Body::empty());
        request.extensions_mut().insert(LocalSocketPeer);
        request.headers_mut().insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_static("203.0.113.9"),
        );

        // Trust everything, to prove the marker short-circuits ahead of the
        // header path rather than merely coinciding with an empty allowlist.
        let resolved = client_addr_from_request(&request, &trusting("0.0.0.0/0"));
        assert_eq!(resolved.source, ClientAddrSource::LocalSocket);
        assert_eq!(resolved.ip, ip("127.0.0.1"));
    }

    /// The counter is the diagnostic half: a wiring regression raises the
    /// process-global total. Other unit tests exercise the same no-peer path
    /// concurrently, so this assertion deliberately checks monotonic progress
    /// instead of a race-prone exact delta.
    #[test]
    fn unobserved_resolutions_are_counted() {
        let counter = UNOBSERVED_RESOLUTIONS
            .as_ref()
            .expect("the test process owns this unique metric name");
        let before = counter.get();
        let _ = resolve_client_addr(None, |_| None, &TrustedProxies::default());
        assert!(
            counter.get() > before,
            "the no-peer path must advance the diagnostic counter"
        );
    }

    #[test]
    fn unobserved_metric_can_be_initialized_before_an_event() {
        initialize_client_addr_metrics();
        let registered = prometheus::gather()
            .iter()
            .any(|family| family.name() == "proximadb_client_addr_unobserved_total");
        assert!(registered, "a healthy process must export an explicit zero");
    }

    /// A TCP surface that forgot `into_make_service_with_connect_info` must be
    /// visibly `Unobserved`, not silently indistinguishable from a local client.
    #[test]
    fn missing_connect_info_reads_as_unobserved() {
        let request = Request::new(axum::body::Body::empty());
        let resolved = client_addr_from_request(&request, &TrustedProxies::default());
        assert_eq!(resolved.source, ClientAddrSource::Unobserved);
        assert!(!resolved.is_observed());
    }

    #[test]
    fn default_allowlist_trusts_nobody() {
        let t = TrustedProxies::default();
        assert!(t.is_empty());
        assert!(!t.trusts(ip("10.0.0.1")));
        assert!(!t.trusts(ip("127.0.0.1")));
    }

    #[test]
    fn allowlist_accepts_cidrs_and_bare_addresses_and_reports_typos() {
        let (t, rejected) = TrustedProxies::parse("10.0.0.0/8, 192.168.1.5 , nonsense, ");
        assert!(t.trusts(ip("10.4.4.4")));
        assert!(t.trusts(ip("192.168.1.5")), "bare address = host route");
        assert!(!t.trusts(ip("192.168.1.6")));
        assert_eq!(rejected, vec!["nonsense".to_string()]);
    }

    /// A typo must narrow trust, never widen it.
    #[test]
    fn a_wholly_unparseable_allowlist_trusts_nobody() {
        let (t, rejected) = TrustedProxies::parse("garbage,also-garbage");
        assert!(t.is_empty());
        assert_eq!(rejected.len(), 2);
        assert!(!t.trusts(ip("10.0.0.1")));
    }
}
