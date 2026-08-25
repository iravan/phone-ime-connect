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
///    skipping VPN/tunnel *and* VM/container/host-only virtual adapters --
///    none of which a phone on the local Wi-Fi can reach. Ties are broken
///    toward the interface carrying the default route, then lowest IP.
/// 3. The routing-table trick (connect a UDP socket at a public address so
///    the kernel picks the outbound local address; no packet is sent), used
///    only if step 2 finds nothing -- and rejected if it lands on a
///    virtual/tunnel adapter.
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
    // Last resort, only reached when no usable physical interface was found.
    // Still reject a probe result that belongs to a virtual/tunnel adapter
    // (e.g. a VM NAT that owns the default route).
    route_probe().filter(|ip| !ip_on_unreachable_interface(*ip))
}

fn env_override() -> Option<Ipv4Addr> {
    std::env::var(LAN_IP_OVERRIDE_ENV)
        .ok()?
        .trim()
        .parse::<Ipv4Addr>()
        .ok()
}

/// Picks a private IPv4 on a physical interface, preferring Wi-Fi/Ethernet
/// (`en*`/`eth*`/`wl*`), skipping VPN/tunnel and VM/container/host-only
/// virtual adapters outright. Among survivors the choice is deterministic:
/// prefer a physical NIC, then the interface carrying the default route
/// (matches `route_probe` -- so a laptop on both Wi-Fi and Ethernet advertises
/// the one that actually reaches the network), then the lowest IP. That last
/// tiebreak matters because `if_addrs`'s enumeration order isn't guaranteed:
/// the previous "first-seen of equal score wins" could advertise a different
/// (wrong) NIC between reboots.
fn physical_lan_interface() -> Option<Ipv4Addr> {
    let route_ip = route_probe();
    if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .filter_map(|iface| match iface.addr {
            if_addrs::IfAddr::V4(v4) => Some((iface.name, v4.ip)),
            if_addrs::IfAddr::V6(_) => None,
        })
        .filter(|(_, ip)| !ip.is_loopback() && !ip.is_link_local() && ip.is_private())
        .filter(|(name, _)| !is_unreachable_virtual(name))
        .max_by_key(|(name, ip)| {
            (
                // Real Wi-Fi/Ethernet beats an unrecognized-but-not-virtual NIC.
                u8::from(is_physical(name)),
                // Then the interface the kernel would actually route out of.
                u8::from(Some(*ip) == route_ip),
                // Final, order-independent tiebreak: lowest IP wins.
                std::cmp::Reverse(u32::from(*ip)),
            )
        })
        .map(|(_, ip)| ip)
}

/// Interfaces whose address a phone on the local Wi-Fi cannot reach: VPN /
/// point-to-point tunnels, and VM / container / host-only virtual adapters
/// (VirtualBox, VMware, Hyper-V/WSL, Docker, libvirt). All are skipped so a
/// virtual adapter's private address is never advertised in the QR code --
/// the reported failure was VirtualBox's host-only `192.168.56.x`, but every
/// one of these is the same class of un-routable-to-the-phone address.
///
/// macOS/Linux kernel interface names (`utun4`, `wg0`, `vboxnet0`, `docker0`,
/// ...) are short, lowercase, and stable, so a prefix match works there.
/// Windows instead reports the adapter's *friendly name* (e.g. "Wi-Fi",
/// "Cisco AnyConnect Secure Mobility Client VPN Adapter", "VMware Network
/// Adapter VMnet8", "vEthernet (WSL)"), which is capitalized, spaced, and
/// puts the identifying word anywhere -- so this also does a case-insensitive
/// substring search for common VPN/VM product names. Necessarily best-effort
/// (there's no programmatic "is this virtual" flag from `if-addrs`) -- the
/// `PHONE_INPUT_CONNECT_LAN_IP` escape hatch above remains the reliable fix
/// for a setup this still gets wrong.
fn is_unreachable_virtual(name: &str) -> bool {
    const SKIP_PREFIXES: [&str; 12] = [
        // VPN / point-to-point tunnels
        "utun", "tun", "tap", "ppp", "ipsec", "wg", "gpd",
        // VM / container / host-only virtual adapters (kernel device names)
        "vboxnet", "vmnet", "docker", "virbr", "veth",
    ];
    if SKIP_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    const SKIP_KEYWORDS: [&str; 22] = [
        // VPN clients (Windows friendly names)
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
        // VM / container / host-only (Windows friendly names)
        "virtualbox",
        "vmware",
        "hyper-v",
        "vethernet", // Hyper-V / WSL virtual switch: "vEthernet (WSL)"
        "host-only",
    ];
    let lower = name.to_ascii_lowercase();
    SKIP_KEYWORDS.iter().any(|k| lower.contains(k))
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

/// Ask the kernel which local address it would route an outbound packet
/// through. `connect()` on a UDP socket sends nothing -- it only consults the
/// routing table. Used both to break ties in `physical_lan_interface` and as
/// the last-resort fallback in `detect_lan_ipv4`.
fn route_probe() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(SocketAddr::from(([8, 8, 8, 8], 80))).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

/// Whether `ip` belongs to a local interface classified as unreachable
/// (VPN/tunnel or VM/container/host-only) -- used to reject a `route_probe`
/// fallback that lands on such an adapter. Returns `false` when the IP isn't
/// found among local interfaces, leaving the last-resort probe as-is.
fn ip_on_unreachable_interface(ip: Ipv4Addr) -> bool {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return false;
    };
    ifaces.iter().any(|iface| {
        matches!(iface.addr, if_addrs::IfAddr::V4(ref v4) if v4.ip == ip)
            && is_unreachable_virtual(&iface.name)
    })
}

#[cfg(test)]
mod tests {
    use super::{is_physical, is_unreachable_virtual};

    #[test]
    fn classifies_interface_names() {
        // macOS VPN tunnels and other point-to-point links are skipped.
        assert!(is_unreachable_virtual("utun4"));
        assert!(is_unreachable_virtual("ipsec0"));
        assert!(is_unreachable_virtual("ppp0"));
        // Real NICs are preferred and are not skipped.
        assert!(is_physical("en0"));
        assert!(is_physical("eth0"));
        assert!(!is_unreachable_virtual("en0"));

        // VM / container / host-only virtual adapters a phone can't reach are
        // skipped outright now, not left as low-priority fallbacks that can
        // win an enumeration-order tie (the reported VirtualBox bug).
        assert!(is_unreachable_virtual("vboxnet0")); // VirtualBox (macOS)
        assert!(is_unreachable_virtual("vmnet8")); // VMware
        assert!(is_unreachable_virtual("docker0")); // Docker
        assert!(is_unreachable_virtual("virbr0")); // libvirt
        assert!(is_unreachable_virtual("veth1a2b3c")); // container veth pair
        assert!(is_unreachable_virtual("VirtualBox Host-Only Ethernet Adapter"));
        assert!(is_unreachable_virtual("VMware Network Adapter VMnet8"));
        assert!(is_unreachable_virtual("vEthernet (WSL)")); // Hyper-V / WSL
        assert!(!is_physical("vEthernet (WSL)"));
        assert!(!is_physical("VirtualBox Host-Only Ethernet Adapter"));

        // A bare macOS internet-sharing bridge stays neutral: not physical,
        // and not matched as virtual (it may be a legitimate bridged LAN).
        assert!(!is_physical("bridge100"));
        assert!(!is_unreachable_virtual("bridge100"));

        // Windows reports friendly names, not kernel device names -- these
        // must still classify correctly even though they're capitalized,
        // contain spaces, and put the identifying word anywhere.
        assert!(is_physical("Wi-Fi"));
        assert!(is_physical("Ethernet 2"));
        assert!(is_physical("Local Area Connection* 9"));
        assert!(is_unreachable_virtual(
            "Cisco AnyConnect Secure Mobility Client VPN Adapter"
        ));
        assert!(is_unreachable_virtual("TAP-Windows Adapter V9"));
        assert!(is_unreachable_virtual("PANGP Virtual Ethernet Adapter"));
        assert!(is_unreachable_virtual("WireGuard Tunnel"));
        assert!(!is_unreachable_virtual("Wi-Fi"));
        // A real "Ethernet N" adapter must not trip the "vethernet" keyword.
        assert!(!is_unreachable_virtual("Ethernet 2"));
    }
}
