//! This machine's IPv4 interfaces, and which one carries traffic to an
//! address: what the host tells the server, and what the client tells the
//! user when the host does not answer (another VPN took the route, etc.).

use std::net::Ipv4Addr;

/// The /24 around `ip`, as a VPN's exclusion list wants it: `192.168.100.0/24`.
pub fn subnet24(ip: Ipv4Addr) -> String {
    let [a, b, c, _] = ip.octets();
    format!("{a}.{b}.{c}.0/24")
}

/// An IPv4 address of an interface that is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub ip: Ipv4Addr,
    /// As Windows shows it (`Ethernet`, `wg0`, the VPN's adapter…).
    pub adapter: String,
}

#[cfg(any(windows, lumen_check))]
mod win {
    use super::Interface;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    const ERROR_BUFFER_OVERFLOW: u32 = 111;

    /// IPv4 addresses of the interfaces that are up (LAN, WireGuard,
    /// Tailscale…), without loopback and link-local ones. Sorted by address.
    pub fn interfaces() -> Vec<Interface> {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size = 16u32 << 10;
        // u64 elements: the list needs 8-byte alignment.
        let mut buf: Vec<u64> = Vec::new();
        for _ in 0..4 {
            buf.resize((size as usize).div_ceil(8), 0);
            let head = buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
            let r = unsafe {
                GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(head), &mut size)
            };
            if r == ERROR_BUFFER_OVERFLOW {
                continue;
            }
            if r != 0 {
                return Vec::new();
            }
            let mut out = Vec::new();
            let mut adapter = head.cast_const();
            while !adapter.is_null() {
                // SAFETY: a list GetAdaptersAddresses wrote into `buf`, alive here.
                let a = unsafe { &*adapter };
                if a.OperStatus == IfOperStatusUp {
                    let name = unsafe { a.FriendlyName.to_string() }.unwrap_or_default();
                    let mut u = a.FirstUnicastAddress.cast_const();
                    while !u.is_null() {
                        let ua = unsafe { &*u };
                        let sa = ua.Address.lpSockaddr;
                        if !sa.is_null() && unsafe { (*sa).sa_family } == AF_INET {
                            let sin = unsafe { &*sa.cast::<SOCKADDR_IN>() };
                            // In network order in memory.
                            let ip =
                                Ipv4Addr::from(unsafe { sin.sin_addr.S_un.S_addr }.to_ne_bytes());
                            if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
                                out.push(Interface {
                                    ip,
                                    adapter: name.clone(),
                                });
                            }
                        }
                        u = ua.Next.cast_const();
                    }
                }
                adapter = a.Next.cast_const();
            }
            out.sort_by_key(|i| i.ip);
            out.dedup_by_key(|i| i.ip);
            return out;
        }
        Vec::new()
    }

    /// [`interfaces`], addresses only.
    pub fn local_ipv4s() -> Vec<Ipv4Addr> {
        interfaces().into_iter().map(|i| i.ip).collect()
    }

    /// The interface Windows would send from to reach `dst` (its routing
    /// table; nothing is sent). None without a route.
    pub fn route_to(dst: Ipv4Addr) -> Option<Interface> {
        let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        s.connect((dst, 9)).ok()?;
        let SocketAddr::V4(local) = s.local_addr().ok()? else {
            return None;
        };
        let ip = *local.ip();
        let adapter = interfaces()
            .into_iter()
            .find(|i| i.ip == ip)
            .map(|i| i.adapter)
            .unwrap_or_default();
        Some(Interface { ip, adapter })
    }
}

#[cfg(any(windows, lumen_check))]
pub use win::{interfaces, local_ipv4s, route_to};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_is_the_slash_24() {
        assert_eq!(
            subnet24(Ipv4Addr::new(192, 168, 100, 41)),
            "192.168.100.0/24"
        );
        assert_eq!(subnet24(Ipv4Addr::new(10, 0, 0, 0)), "10.0.0.0/24");
    }
}
