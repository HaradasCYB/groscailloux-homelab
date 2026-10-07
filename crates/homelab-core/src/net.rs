//! Réseaux en notation CIDR (`172.18.0.0/16`, `fd00::/8`, ou une adresse seule) : `[web] trusted_proxies`,
//! les pairs TCP dont homelabd croit l'en-tête `X-Forwarded-For`.

use std::fmt;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// `a.b.c.d/n`, `x::/n` ou une adresse seule (/32, /128). Les bits d'hôte sont ignorés
    /// (`172.18.0.1/16` = `172.18.0.0/16`).
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a
            .trim()
            .parse()
            .map_err(|_| format!("adresse invalide : {s:?}"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            None => max,
            Some(p) => p
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|n| *n <= max)
                .ok_or_else(|| format!("préfixe invalide : {s:?}"))?,
        };
        Ok(Self { addr, prefix })
    }

    /// `ip` est dans ce réseau ? Une adresse IPv4 vue en IPv6 (`::ffff:172.18.0.19`) compte comme IPv4.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn docker_network_contains_its_containers_only() {
        let n = Cidr::parse("172.18.0.0/16").unwrap();
        assert!(n.contains(ip("172.18.0.19")));
        assert!(n.contains(ip("172.18.255.254")));
        assert!(!n.contains(ip("172.19.0.1")));
        assert!(!n.contains(ip("127.0.0.1")));
        // vue IPv6 d'une adresse IPv4 (écoute double pile)
        assert!(n.contains(ip("::ffff:172.18.0.19")));
        assert!(!n.contains(ip("fd00::1")));
    }

    #[test]
    fn single_address_and_edge_prefixes() {
        let one = Cidr::parse("172.18.0.19").unwrap();
        assert!(one.contains(ip("172.18.0.19")));
        assert!(!one.contains(ip("172.18.0.20")));
        let all = Cidr::parse("0.0.0.0/0").unwrap();
        assert!(all.contains(ip("203.0.113.9")));
        let v6 = Cidr::parse("fd00::/8").unwrap();
        assert!(v6.contains(ip("fd12:3456::1")));
        assert!(!v6.contains(ip("fe80::1")));
        // bits d'hôte ignorés
        assert!(Cidr::parse("172.18.0.1/16")
            .unwrap()
            .contains(ip("172.18.9.9")));
        assert_eq!(
            Cidr::parse(" 10.0.0.0/8 ").unwrap().to_string(),
            "10.0.0.0/8"
        );
    }

    #[test]
    fn garbage_is_refused() {
        for bad in [
            "",
            "npm",
            "172.18.0.0/33",
            "172.18.0.0/",
            "::/129",
            "1.2.3/8",
        ] {
            assert!(Cidr::parse(bad).is_err(), "{bad:?}");
        }
    }
}
