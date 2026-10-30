//! SSRF guard for server-side fetches of admin-supplied URLs.
//!
//! With `allow_private = false`, requests to loopback, private (RFC 1918 / CGNAT / ULA),
//! link-local (incl. cloud metadata `169.254.169.254`), multicast and unspecified addresses are
//! refused — for literal IPs, for every address a hostname resolves to (DNS-rebinding safe,
//! because filtering happens in the resolver used for the actual connection), and for every
//! redirect hop.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};

use crate::{Error, Result};

/// True for addresses a server-side fetch must not reach in strict mode.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return blocked_v4(v4);
            }
            blocked_v6(v6)
        }
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10 carrier-grade NAT / Tailscale
        || o[0] == 0
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || o[0] >= 240
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg[0] & 0xFE00) == 0xFC00 // unique local fc00::/7
        || (seg[0] & 0xFFC0) == 0xFE80 // link-local fe80::/10
        || (seg[0] == 0x2001 && seg[1] == 0x0DB8) // documentation
}

fn host_blocked(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".internal")
    {
        return true;
    }
    h.parse::<IpAddr>().is_ok_and(is_blocked_ip)
}

/// Cheap pre-flight check of a URL's literal host (names are enforced by the resolver).
pub fn check_url(url: &str, allow_private: bool) -> Result<()> {
    let u = reqwest::Url::parse(url).map_err(|e| Error::Invalid(format!("invalid URL: {e}")))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(Error::Invalid(
            "only http:// and https:// URLs are allowed".into(),
        ));
    }
    let host = u
        .host_str()
        .ok_or_else(|| Error::Invalid("URL has no host".into()))?;
    if !allow_private && host_blocked(host) {
        return Err(Error::Invalid(format!(
            "fetching from {host} is blocked (private/loopback/link-local address); \
             set allow_private_fetch = true to allow LAN sources"
        )));
    }
    Ok(())
}

/// Resolver that drops blocked addresses.
struct GuardResolver;

impl Resolve for GuardResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .filter(|a| !is_blocked_ip(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!(
                    "{host} resolves only to private/loopback/link-local addresses (blocked)"
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// An HTTP client for fetching admin-supplied URLs under the given policy.
pub fn client(allow_private: bool) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .tcp_nodelay(true);
    b = if allow_private {
        b.redirect(redirect::Policy::limited(10))
    } else {
        b.dns_resolver(Arc::new(GuardResolver))
            .redirect(redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 10 {
                    return attempt.error("too many redirects");
                }
                let blocked = attempt.url().host_str().is_some_and(host_blocked);
                if blocked {
                    attempt.error("redirect to a private/loopback address is blocked")
                } else {
                    attempt.follow()
                }
            }))
    };
    b.build().expect("reqwest client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "100.127.255.255",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "ff02::1",
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "{ip} should be blocked");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(
                !is_blocked_ip(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[test]
    fn url_checks() {
        assert!(check_url("http://127.0.0.1:8080/x", true).is_ok());
        assert!(check_url("http://127.0.0.1:8080/x", false).is_err());
        assert!(check_url("http://localhost/x", false).is_err());
        assert!(check_url("http://[::1]/x", false).is_err());
        assert!(check_url("http://169.254.169.254/latest/meta-data", false).is_err());
        assert!(check_url("http://nas.local/list.m3u", false).is_err());
        assert!(check_url("https://example.com/list.m3u", false).is_ok());
        assert!(check_url("file:///etc/passwd", true).is_err());
        assert!(check_url("ftp://x/y", true).is_err());
        let e = check_url("http://10.0.0.5/x", false)
            .unwrap_err()
            .to_string();
        assert!(e.contains("allow_private_fetch"), "{e}");
    }

    #[tokio::test]
    async fn strict_client_refuses_loopback_by_name_and_literal() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    // Read the request head first; answering before the client has written its
                    // request makes hyper fail with UnexpectedMessage on Linux.
                    let mut buf = [0u8; 1024];
                    let mut seen = Vec::new();
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = s
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                        )
                        .await;
                });
            }
        });
        let strict = client(false);
        let lax = client(true);
        // `localhost` resolves to loopback: the guarded resolver leaves nothing to connect to
        let err = strict
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .unwrap_err();
        assert!(err.is_connect(), "{err:?}");
        assert_eq!(
            lax.get(format!("http://localhost:{port}/"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        assert!(check_url(&format!("http://127.0.0.1:{port}/"), false).is_err());
    }
}
