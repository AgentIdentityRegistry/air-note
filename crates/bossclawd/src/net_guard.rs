// Copied from apps/desktop/src-tauri/src/web_access.rs (`is_blocked_ip`, M1a Task 4);
// the in-app original is removed in Task 6. The cloud reasoner's connect-time SSRF
// pin (`engine::cloud_reasoner::PinnedResolver` → `screen_addrs`) screens every
// resolved address through this before opening a socket. Self-contained (std::net only).

use std::net::{IpAddr, Ipv4Addr};

/// SSRF guard: `true` when `ip` is NOT a safe public destination — loopback, private,
/// link-local (incl. the `169.254.169.254` cloud-metadata endpoint), CGNAT, benchmarking,
/// reserved, unspecified, broadcast, multicast, or documentation space. IPv4-in-IPv6
/// forms are unwrapped so `::ffff:127.0.0.1` (and `::1`) are caught too.
pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || octets[0] == 0 // 0.0.0.0/8 "this network"
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
                || (octets[0] == 198 && (octets[1] & 0xfe) == 18) // 198.18.0.0/15 benchmarking
                || octets[0] >= 240 // 240.0.0.0/4 reserved (Ipv4Addr::is_reserved is unstable)
        }
        IpAddr::V6(v6) => {
            // Unwrap IPv4-mapped (::ffff:a.b.c.d) / IPv4-compatible (::a.b.c.d, incl. ::1).
            if let Some(v4) = v6.to_ipv4() {
                if is_blocked_ip(IpAddr::V4(v4)) {
                    return true;
                }
            }
            let segments = v6.segments();
            // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 in the low 32 bits; a NAT64
            // gateway translates it to that v4, so unwrap and re-check the embedded address.
            if segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0] {
                let embedded = Ipv4Addr::new(
                    (segments[6] >> 8) as u8,
                    (segments[6] & 0xff) as u8,
                    (segments[7] >> 8) as u8,
                    (segments[7] & 0xff) as u8,
                );
                if is_blocked_ip(IpAddr::V4(embedded)) {
                    return true;
                }
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (segments[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}
