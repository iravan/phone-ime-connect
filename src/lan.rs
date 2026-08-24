//! Detects the LAN-facing IPv4 address that phones on the same network can
//! use to reach this machine.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// Set this to a specific IPv4 address to skip auto-detection entirely --
/// an escape hatch for setups the heuristics below get wrong.
const LAN_IP_OVERRIDE_ENV: &str = "PHONE_INPUT_CONNECT_LAN_IP";

/// Best-effort discovery of this machine's LAN IPv4 address -- the one a
/// phone on the same Wi-Fi/LAN can reach.
///
/// Order of preference:
/// 1. `PHONE_INPUT_CONNECT_LAN_IP`, if set to a valid IPv4 address.
/// 2. A private-range address on a real physical interface (Wi-Fi/Ethernet),
///    explicitly skipping VPN/tunnel interfaces -- otherwise a VPN that owns
///    the default route hands us its own address, which no phone can reach.
/// 3. The routing-table trick (connect a UDP socket at a public address so
///    the kernel picks the outbound local address; no packet is sent). This
///    is the old behaviour, kept as a fallback.
///
/// Returns `None` if nothing usable is found (or it resolves to loopback) --
/// callers should treat that as "refuse to start", not "listen on every
/// interface".
pub fn detect_lan_ipv4() -> Option<Ipv4Addr> {
    if let Some(ip) = env_override() {
        return Some(ip);
    }
    if let Some(ip) = physical_lan_interface() {
        return Some(ip);
    }
    route_probe()
}

fn env_override() -> Option<Ipv4Addr> {
    std::env::var(LAN_IP_OVERRIDE_ENV)
        .ok()?
        .trim()
        .parse::<Ipv4Addr>()
        .ok()
}

/// Picks a private IPv4 on a physical interface, preferring Wi-Fi/Ethernet
/// (`en*`/`eth*`/`wl*`) over other non-tunnel interfaces (e.g. bridges), and
/// skipping VPN/tunnel interfaces outright.
fn physical_lan_interface() -> Option<Ipv4Addr> {
    let ifaces = if_addrs::get_if_addrs().ok()?;
    let mut best: Option<(u8, Ipv4Addr)> = None;
    for iface in ifaces {
        let ip = match iface.addr {
            if_addrs::IfAddr::V4(v4) => v4.ip,
            if_addrs::IfAddr::V6(_) => continue,
        };
        if ip.is_loopback() || ip.is_link_local() || !ip.is_private() {
            continue;
        }
        if is_tunnel(&iface.name) {
            continue;
        }
        // Prefer a real Wi-Fi/Ethernet NIC over host-only virtual bridges.
        let score = if is_physical(&iface.name) { 2 } else { 1 };
        if best.map_or(true, |(best_score, _)| score > best_score) {
            best = Some((score, ip));
        }
    }
    best.map(|(_, ip)| ip)
}

/// VPN / point-to-point tunnel interface names, whose address a phone on the
/// local Wi-Fi cannot reach.
///
/// macOS/Linux kernel interface names (`utun4`, `ipsec0`, `wg0`, ...) are
/// short, lowercase, and stable, so a prefix match was enough there. Windows
/// is different: `if-addrs` reports the adapter's *friendly name* on
/// Windows (e.g. "Wi-Fi", "Ethernet 2", "Cisco AnyConnect Secure Mobility
/// Client VPN Adapter"), which is capitalized, can contain spaces, and puts
/// the identifying word anywhere in the string rather than at the start --
/// a plain `starts_with` on a lowercase prefix never matches any of that.
/// So this also does a case-insensitive substring search for common VPN
/// client names. This list is necessarily best-effort (there is no
/// programmatic "is this a VPN adapter" flag exposed by `if-addrs`, and it
/// hasn't been tested against a real VPN client on Windows) -- the
/// `PHONE_INPUT_CONNECT_LAN_IP` escape hatch above remains the reliable
/// fix for a setup this still gets wrong.
fn is_tunnel(name: &str) -> bool {
    const TUNNEL_PREFIXES: [&str; 7] = ["utun", "tun", "tap", "ppp", "ipsec", "wg", "gpd"];
    if TUNNEL_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    const TUNNEL_KEYWORDS: [&str; 17] = [
        "vpn",
        "tap-windows",
        "wireguard",
        "openvpn",
        "tailscale",
        "zerotier",
        "globalprotect",
        "pangp", // Palo Alto GlobalProtect's virtual adapter is typically "PANGP ..."
        "anyconnect",
        "pulse secure",
        "junos pulse",
        "checkpoint",
        "forticlient",
        "zscaler",
        "sonicwall",
        "hamachi",
        "softether",
    ];
    let lower = name.to_ascii_lowercase();
    TUNNEL_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Physical Wi-Fi / Ethernet interface names: macOS/Linux kernel names
/// (`en*`, `eth*`, `wl*`) plus Windows friendly names ("Wi-Fi", "Ethernet",
/// "Local Area Connection"), matched case-insensitively -- see `is_tunnel`
/// for why Windows needs different handling than the other two platforms.
fn is_physical(name: &str) -> bool {
    const PHYSICAL_PREFIXES: [&str; 3] = ["en", "eth", "wl"];
    if PHYSICAL_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    const PHYSICAL_PREFIXES_CI: [&str; 3] = ["wi-fi", "ethernet", "local area connection"];
    let lower = name.to_ascii_lowercase();
    PHYSICAL_PREFIXES_CI.iter().any(|p| lower.starts_with(p))
}

/// Fallback: ask the kernel which local address it would route an outbound
/// packet through. `connect()` on a UDP socket sends nothing -- it only
/// consults the routing table.
fn route_probe() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(SocketAddr::from(([8, 8, 8, 8], 80))).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_physical, is_tunnel};

    #[test]
    fn classifies_interface_names() {
        // macOS VPN tunnels and other point-to-point links are skipped.
        assert!(is_tunnel("utun4"));
        assert!(is_tunnel("ipsec0"));
        assert!(is_tunnel("ppp0"));
        // Real NICs are preferred and are not tunnels.
        assert!(is_physical("en0"));
        assert!(is_physical("eth0"));
        assert!(!is_tunnel("en0"));
        // Host-only virtual bridges are neither physical nor tunnels, so
        // they stay as low-priority fallbacks.
        assert!(!is_physical("bridge100"));
        assert!(!is_tunnel("bridge100"));

        // Windows reports friendly names, not kernel device names -- these
        // must still classify correctly even though they're capitalized,
        // contain spaces, and put the identifying word anywhere.
        assert!(is_physical("Wi-Fi"));
        assert!(is_physical("Ethernet 2"));
        assert!(is_physical("Local Area Connection* 9"));
        assert!(is_tunnel("Cisco AnyConnect Secure Mobility Client VPN Adapter"));
        assert!(is_tunnel("TAP-Windows Adapter V9"));
        assert!(is_tunnel("PANGP Virtual Ethernet Adapter"));
        assert!(is_tunnel("WireGuard Tunnel"));
        assert!(!is_tunnel("Wi-Fi"));
        assert!(!is_tunnel("Ethernet 2"));
    }
}
