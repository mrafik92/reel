//! Small network helpers: CIDR matching and private-range checks (client-IP trust, setup guard).

use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    ip: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// `10.0.0.0/8`, `fd00::/8` or a bare address (host route).
    pub fn parse(s: &str) -> Option<Cidr> {
        let s = s.trim();
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p.parse::<u8>().ok()?)),
            None => (s, None),
        };
        let ip: IpAddr = a.parse().ok()?;
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let prefix = p.unwrap_or(max);
        (prefix <= max).then_some(Cidr { ip, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.ip, normalize(ip)) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let m = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(self.prefix))
                };
                u32::from(n) & m == u32::from(a) & m
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let m = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(self.prefix))
                };
                u128::from(n) & m == u128::from(a) & m
            }
            _ => false,
        }
    }
}

/// IPv4-mapped IPv6 addresses count as their IPv4 form.
pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// Loopback, RFC 1918, link-local, unique-local: "somewhere on the LAN".
pub fn is_private(ip: IpAddr) -> bool {
    match normalize(ip) {
        IpAddr::V4(a) => a.is_private() || a.is_loopback() || a.is_link_local(),
        IpAddr::V6(a) => {
            a.is_loopback()
                || (a.segments()[0] & 0xfe00) == 0xfc00
                || (a.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Where admin powers may be used (#127): the LAN, or Tailscale's CGNAT range `100.64.0.0/10`.
/// Tailscale's IPv6 range is unique-local, so [`is_private`] already covers it.
pub fn is_admin_network(ip: IpAddr) -> bool {
    let ip = normalize(ip);
    is_private(ip) || is_tailscale(ip)
}

/// Tailscale's address ranges: CGNAT `100.64.0.0/10` and the unique-local `fd7a:115c:a1e0::/48`.
pub fn is_tailscale(ip: IpAddr) -> bool {
    match normalize(ip) {
        IpAddr::V4(a) => a.octets()[0] == 100 && (a.octets()[1] & 0xc0) == 64,
        IpAddr::V6(a) => a.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Is this cast sender on the server's own LAN, where TVs can reach `cast_base_url` (#309)?
///
/// `networks` empty: any private address except Tailscale's. Otherwise the address must match a
/// plain entry and no `!` entry (unparsable entries are ignored).
pub fn on_cast_lan(networks: &[String], ip: IpAddr) -> bool {
    if networks.is_empty() {
        return is_private(ip) && !is_tailscale(ip);
    }
    let (mut inside, mut excluded) = (false, false);
    for n in networks {
        match n.trim().strip_prefix('!') {
            Some(x) => excluded |= Cidr::parse(x).is_some_and(|c| c.contains(ip)),
            None => inside |= Cidr::parse(n).is_some_and(|c| c.contains(ip)),
        }
    }
    inside && !excluded
}

pub fn parse_cidrs(list: &[String]) -> Vec<Cidr> {
    list.iter().filter_map(|s| Cidr::parse(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_network_is_lan_and_tailscale_only() {
        for ip in [
            "10.10.10.5",
            "192.168.1.2",
            "127.0.0.1",
            "100.64.0.1",
            "100.127.255.254",
            "fd7a:115c:a1e0::1",
            "::ffff:100.100.1.1",
        ] {
            assert!(is_admin_network(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "100.63.255.255",
            "100.128.0.1",
            "8.8.8.8",
            "203.0.113.9",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_admin_network(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn cast_lan_defaults_to_private_minus_tailscale_and_honours_exclusions() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        for lan in ["10.10.10.5", "192.168.1.2", "fd00::1", "::ffff:10.0.0.1"] {
            assert!(on_cast_lan(&[], ip(lan)), "{lan}");
        }
        for away in [
            "100.101.1.2",
            "fd7a:115c:a1e0::5",
            "85.144.240.8",
            "2001:db8::1",
        ] {
            assert!(!on_cast_lan(&[], ip(away)), "{away}");
        }
        let home: Vec<String> = ["10.10.10.0/24", "!10.10.10.1", "junk"]
            .map(String::from)
            .into();
        assert!(on_cast_lan(&home, ip("10.10.10.42")));
        assert!(!on_cast_lan(&home, ip("10.10.10.1")), "Tailscale, routed");
        assert!(!on_cast_lan(&home, ip("192.168.1.2")), "not listed");
    }

    #[test]
    fn cidr_and_private() {
        let c = Cidr::parse("10.1.0.0/16").unwrap();
        assert!(c.contains("10.1.200.3".parse().unwrap()));
        assert!(!c.contains("10.2.0.1".parse().unwrap()));
        assert!(Cidr::parse("::1").unwrap().contains("::1".parse().unwrap()));
        assert!(
            Cidr::parse("0.0.0.0/0")
                .unwrap()
                .contains("8.8.8.8".parse().unwrap())
        );
        assert!(Cidr::parse("10.0.0.0/33").is_none() && Cidr::parse("nope").is_none());
        assert!(
            Cidr::parse("10.0.0.0/8")
                .unwrap()
                .contains("::ffff:10.1.1.1".parse().unwrap())
        );
        for p in [
            "127.0.0.1",
            "192.168.1.5",
            "172.17.0.2",
            "169.254.1.1",
            "::1",
            "fd12::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_private(p.parse().unwrap()), "{p}");
        }
        for p in ["8.8.8.8", "2001:db8::1", "172.32.0.1"] {
            assert!(!is_private(p.parse().unwrap()), "{p}");
        }
    }
}
