//! Desktop-side cloud reasoner (Anthropic + OpenAI-compat). The brain's first
//! deliberate network egress: off-by-default, fail-closed, host-pinned, signed
//! consent. Lives here (not bossclaw-core) because the engine crate's CI jail
//! forbids `reqwest`. See docs/superpowers/specs/2026-06-30-milestone-d2-cloud-reasoner-design.md §8.

// Implemented task-by-task in the Phase 2a plan.

use std::net::{SocketAddr, ToSocketAddrs};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::web_access::is_blocked_ip;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Pure SSRF screen: returns the addresses only if EVERY one is a safe public
/// destination; errors if any is internal/loopback/link-local/private/CGNAT/
/// metadata, or if the set is empty. Used at connect time to close the
/// DNS-rebind race that a pre-flight host check cannot (spec §8 R2).
// Exercised by `tests::screen_addrs_rejects_any_blocked` and called from
// `PinnedResolver::resolve`; the bin target compiles without `cfg(test)`, where
// `PinnedResolver` is itself dead until the Task 5 client wires it in.
#[allow(dead_code)]
fn screen_addrs(addrs: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, BoxError> {
    if addrs.is_empty() {
        return Err("cloud reasoner DNS resolved to no addresses".into());
    }
    if let Some(bad) = addrs.iter().find(|a| is_blocked_ip(a.ip())) {
        return Err(format!(
            "cloud reasoner refusing connection: host resolves to a blocked address ({})",
            bad.ip()
        )
        .into());
    }
    Ok(addrs)
}

/// A `reqwest` DNS resolver that screens every resolved address through
/// `is_blocked_ip` before any socket is opened. This is the connect-time pin
/// that closes the rebind race (`web_access.rs:171` documents the residual gap
/// this fills); installed on the blocking client built in Task 5.
#[allow(dead_code)] // Constructed by the Task 5 client builder (Arc::new(PinnedResolver)).
struct PinnedResolver;

impl Resolve for PinnedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let screened = tokio::task::spawn_blocking(move || {
                // Port is irrelevant for screening (reqwest applies the URL's
                // port when connecting); 443 mirrors web_access::validate_host.
                let resolved: Vec<SocketAddr> = (host.as_str(), 443u16)
                    .to_socket_addrs()
                    .map_err(|e| -> BoxError { Box::new(e) })?
                    .collect();
                screen_addrs(resolved)
            })
            .await
            .map_err(|e| -> BoxError { Box::new(e) })??;
            let iter: Addrs = Box::new(screened.into_iter());
            Ok(iter)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn sa(ip: [u8; 4]) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])), 443)
    }

    #[test]
    fn screen_addrs_rejects_any_blocked() {
        // All public -> Ok, preserved in order.
        let public = vec![sa([93, 184, 216, 34]), sa([1, 1, 1, 1])];
        assert!(screen_addrs(public.clone()).is_ok());

        // Loopback present -> Err (DNS-rebind primitive refused before connect).
        let with_loopback = vec![sa([93, 184, 216, 34]), sa([127, 0, 0, 1])];
        assert!(screen_addrs(with_loopback).is_err());

        // Cloud metadata -> Err.
        assert!(screen_addrs(vec![sa([169, 254, 169, 254])]).is_err());

        // Empty -> Err (nothing to connect to).
        assert!(screen_addrs(Vec::new()).is_err());
    }
}
