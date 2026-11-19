//! Which request header names the remote client, for per-client rate limits.
//!
//! Brainkeep listens behind a reverse proxy, so the TCP peer is the proxy. The proxy
//! must overwrite this header with the real client address on every request:
//! Cloudflare does so with `CF-Connecting-IP` (the default), Railway's edge with
//! `X-Real-IP`. Without a single valid value, clients share one bucket.
use axum::http::{HeaderMap, HeaderName};
use std::{net::IpAddr, sync::OnceLock};

pub const DEFAULT: &str = "cf-connecting-ip";
static HEADER: OnceLock<HeaderName> = OnceLock::new();

/// Sets the header once at startup. Rejects names that carry credentials or routing.
pub fn configure(name: &str) -> Result<(), String> {
    let header =
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| "invalid client IP header name")?;
    if matches!(
        header.as_str(),
        "authorization" | "cookie" | "host" | "origin" | "content-length" | "content-type"
    ) {
        return Err("client IP header cannot be a request control header".into());
    }
    // Lists never parse as one address: every client would share one bucket.
    if matches!(header.as_str(), "x-forwarded-for" | "forwarded") {
        return Err("client IP header must hold a single address, e.g. X-Real-IP".into());
    }
    HEADER
        .set(header)
        .map_err(|_| "client IP header already configured".into())
}

fn header() -> &'static HeaderName {
    HEADER.get_or_init(|| HeaderName::from_static(DEFAULT))
}

/// The client address when the header holds exactly one valid IP. IPv4-mapped IPv6
/// addresses count as their IPv4 address.
pub fn get(headers: &HeaderMap) -> Option<IpAddr> {
    let mut values = headers.get_all(header()).iter();
    match (values.next(), values.next()) {
        (Some(v), None) => v
            .to_str()
            .ok()?
            .trim()
            .parse::<IpAddr>()
            .ok()
            .map(|ip| ip.to_canonical()),
        _ => None,
    }
}
