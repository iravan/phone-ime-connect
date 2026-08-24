//! Detects the LAN-facing IPv4 address that phones on the same network can
//! use to reach this machine. Pure `std::net` -- identical on Windows,
//! macOS, and Linux.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// Best-effort discovery of this machine's outbound LAN IPv4 address.
///
/// Opens a UDP socket and "connects" it to a public address purely so the
/// kernel resolves which local interface/address it would route through --
/// no packet is actually sent for a UDP connect(), it only consults the
/// routing table. That local address is normally the one other devices on
/// the same Wi-Fi/LAN segment can reach us at. Returns `None` if it
/// resolves to loopback (no real network interface) or the lookup fails
/// outright -- callers should treat that as "refuse to start", not "fall
/// back to listening on every interface".
pub fn detect_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(SocketAddr::from(([8, 8, 8, 8], 80))).ok()?;
    let ip = match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) => v4,
        std::net::IpAddr::V6(_) => return None,
    };
    if ip.is_loopback() {
        return None;
    }
    Some(ip)
}
