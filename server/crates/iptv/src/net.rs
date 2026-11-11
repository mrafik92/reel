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
///
/// IPv6 forms that carry an IPv4 address (mapped `::ffff:a.b.c.d`, translated `::ffff:0:a.b.c.d`,
/// NAT64 `64:ff9b::a.b.c.d`, 6to4 `2002:AABB:CCDD::`, Teredo `2001:0::`) are judged by the
/// embedded IPv4 address, so `64:ff9b::127.0.0.1` on a NAT64 network cannot reach loopback.
/// Deprecated IPv4-compatible `::a.b.c.d` and local-use NAT64 `64:ff9b:1::/48` are refused outright.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => blocked_v6(v6),
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
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24 IETF protocol assignments
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || o[0] >= 240
}

fn v4_from(hi: u16, lo: u16) -> Ipv4Addr {
    let [a, b] = hi.to_be_bytes();
    let [c, d] = lo.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    // ::ffff:a.b.c.d (mapped) and ::ffff:0:a.b.c.d (translated, RFC 2765)
    if seg[..4] == [0; 4]
        && ((seg[4] == 0 && seg[5] == 0xFFFF) || (seg[4] == 0xFFFF && seg[5] == 0))
    {
        return blocked_v4(v4_from(seg[6], seg[7]));
    }
    // 64:ff9b::/96 well-known NAT64 prefix (RFC 6052): the IPv4 address it translates to
    if seg[..6] == [0x64, 0xFF9B, 0, 0, 0, 0] {
        return blocked_v4(v4_from(seg[6], seg[7]));
    }
    // 2002::/16 6to4: the IPv4 address of the relay router
    if seg[0] == 0x2002 {
        return blocked_v4(v4_from(seg[1], seg[2]));
    }
    // 2001:0::/32 Teredo: server address in clear, client address bit-inverted
    if seg[0] == 0x2001 && seg[1] == 0 {
        return blocked_v4(v4_from(seg[2], seg[3])) || blocked_v4(v4_from(!seg[6], !seg[7]));
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || seg[..6] == [0; 6] // ::/96 IPv4-compatible (deprecated): never a legitimate peer
        || (seg[0] == 0x64 && seg[1] == 0xFF9B && seg[2] == 1) // 64:ff9b:1::/48 local-use NAT64
        || (seg[0] == 0x0100 && seg[1..4] == [0; 3]) // 100::/64 discard-only
        || (seg[0] & 0xFE00) == 0xFC00 // unique local fc00::/7
        || (seg[0] & 0xFFC0) == 0xFE80 // link-local fe80::/10
        || (seg[0] & 0xFFC0) == 0xFEC0 // site-local fec0::/10 (deprecated)
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

/// Player-like User-Agent used for every provider request unless a source overrides it.
pub const DEFAULT_UA: &str = "VLC/3.0.20 LibVLC/3.0.20";

const SECRET_PARAMS: &[&str] = &[
    "username", "password", "user", "pass", "pwd", "token", "api_key", "apikey", "key", "secret",
    "auth",
];

/// Mask credentials in a URL: userinfo, secret query parameters and the `user/pass` path segments
/// of Xtream stream URLs (`/live/USER/PASS/123.ts`).
pub fn redact_url(url: &str) -> String {
    let Ok(mut u) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    if !u.username().is_empty() || u.password().is_some() {
        let _ = u.set_username("***");
        let _ = u.set_password(Some("***"));
    }
    if u.query().is_some() {
        let pairs: Vec<(String, String)> = u
            .query_pairs()
            .map(|(k, v)| {
                let secret = SECRET_PARAMS.contains(&k.to_ascii_lowercase().as_str());
                (
                    k.into_owned(),
                    if secret { "***".into() } else { v.into_owned() },
                )
            })
            .collect();
        // `***` must stay literal: build the query by hand
        let q: Vec<String> = pairs
            .iter()
            .map(|(k, v)| {
                if v == "***" {
                    format!("{k}=***")
                } else {
                    format!("{k}={}", form_enc(v))
                }
            })
            .collect();
        u.set_query(Some(&q.join("&")));
    }
    let segs: Vec<String> = u
        .path_segments()
        .map_or(vec![], |s| s.map(str::to_string).collect());
    if segs.len() >= 4 && matches!(segs[0].as_str(), "live" | "movie" | "series" | "timeshift") {
        let mut s = segs;
        s[1] = "***".into();
        s[2] = "***".into();
        u.set_path(&s.join("/"));
    }
    u.to_string()
}

fn form_enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Redact every `http(s)://…` URL inside free text (error messages, ffmpeg stderr).
pub fn redact_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("http://").or_else(|| rest.find("https://")) {
        // earliest of the two
        let j = match (rest.find("http://"), rest.find("https://")) {
            (Some(a), Some(b)) => a.min(b),
            _ => i,
        };
        out.push_str(&rest[..j]);
        let tail = &rest[j..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
            .unwrap_or(tail.len());
        let mut url = &tail[..end];
        // trailing punctuation / closing paren belong to the sentence, not the URL
        let trimmed = url.trim_end_matches([')', ',', '.', ';', ']']);
        let suffix = &url[trimmed.len()..];
        url = trimmed;
        out.push_str(&redact_url(url));
        out.push_str(suffix);
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// An HTTP client for fetching admin-supplied URLs under the given policy.
pub fn client(allow_private: bool) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .user_agent(DEFAULT_UA)
        .connect_timeout(Duration::from_secs(15))
        .tcp_nodelay(true)
        // reqwest would send the previous URL as `Referer` on a redirect: that URL often carries
        // the provider login (`/live/USER/PASS/…`, `?username=…`), and it would also replace a
        // source's configured referrer
        .referer(false);
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
            // IPv4-compatible (deprecated) ::a.b.c.d, whatever it embeds
            "::127.0.0.1",
            "::10.0.0.1",
            "::8.8.8.8",
            "::2",
            // translated ::ffff:0:a.b.c.d
            "::ffff:0:127.0.0.1",
            // NAT64 64:ff9b::/96 to a blocked IPv4 address, and local-use NAT64
            "64:ff9b::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::192.168.1.1",
            "64:ff9b:1::8.8.8.8",
            // 6to4 2002::/16 around a blocked IPv4 address
            "2002:7f00:1::1",
            "2002:c0a8:101::1",
            "2002:a9fe:a9fe::",
            // Teredo with a loopback server / a private (bit-inverted) client
            "2001:0:7f00:1::1",
            "2001:0:808:808::f5ff:fffe", // client 10.0.0.1
            // 192.0.0.0/24 IETF protocol assignments (e.g. 192.0.0.170 NAT64 discovery)
            "192.0.0.1",
            "192.0.0.170",
            "::ffff:192.0.0.8",
            // discard-only, site-local
            "100::1",
            "fec0::1",
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
            // NAT64 / 6to4 / Teredo to public IPv4 hosts stay reachable (IPv6-only networks)
            "64:ff9b::8.8.8.8",
            "64:ff9b::101:101",
            "2002:808:808::1",
            "2001:0:808:808::f7f7:f7f7",
            // neighbours of 192.0.0.0/24
            "192.0.1.1",
            "191.255.255.255",
        ] {
            assert!(
                !is_blocked_ip(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[test]
    fn redaction_masks_credentials_everywhere() {
        assert_eq!(
            redact_url("http://p.example:8080/xmltv.php?username=bob&password=s3cr%21t"),
            "http://p.example:8080/xmltv.php?username=***&password=***"
        );
        assert_eq!(
            redact_url(
                "http://p.example/player_api.php?username=u&password=p&action=get_live_streams"
            ),
            "http://p.example/player_api.php?username=***&password=***&action=get_live_streams"
        );
        assert_eq!(
            redact_url("http://p.example:80/live/bob/s3cret/123.ts"),
            "http://p.example/live/***/***/123.ts"
        );
        assert_eq!(
            redact_url("https://user:pw@host.example/list.m3u?x=1"),
            "https://***:***@host.example/list.m3u?x=1"
        );
        assert_eq!(
            redact_url("https://host.example/a/b.m3u8"),
            "https://host.example/a/b.m3u8"
        );
        let msg = redact_text(
            "error sending request for url (http://h.example/get.php?username=a&password=b): timed out; also https://x.example/live/u/p/1.ts, fine",
        );
        assert!(
            !msg.contains("=a&") && !msg.contains("/u/p/") && !msg.contains("password=b"),
            "{msg}"
        );
        assert!(
            msg.contains("timed out") && msg.contains("password=***"),
            "{msg}"
        );
        assert_eq!(redact_text("no urls here"), "no urls here");
    }

    #[test]
    fn url_checks() {
        assert!(check_url("http://127.0.0.1:8080/x", true).is_ok());
        assert!(check_url("http://127.0.0.1:8080/x", false).is_err());
        assert!(check_url("http://localhost/x", false).is_err());
        assert!(check_url("http://[::1]/x", false).is_err());
        assert!(check_url("http://[::127.0.0.1]/x", false).is_err());
        assert!(check_url("http://[64:ff9b::7f00:1]/x", false).is_err());
        assert!(check_url("http://[2002:7f00:1::]/x", false).is_err());
        assert!(check_url("http://192.0.0.8/x", false).is_err());
        assert!(check_url("http://[64:ff9b::808:808]/x", false).is_ok());
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

    #[tokio::test]
    async fn redirects_do_not_leak_the_login_url_as_referer() {
        use axum::{Router, http::HeaderMap, response::Redirect, routing::get};
        let app = Router::new()
            .route(
                "/live/bob/s3cret/1.m3u8",
                get(|| async { Redirect::temporary("/lb/1.m3u8") }),
            )
            .route(
                "/lb/1.m3u8",
                get(|h: HeaderMap| async move {
                    h.get("referer")
                        .map(|v| v.to_str().unwrap_or("?").to_string())
                        .unwrap_or_else(|| "none".into())
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let url = format!("http://{addr}/live/bob/s3cret/1.m3u8");
        let c = client(true);
        assert_eq!(
            c.get(&url).send().await.unwrap().text().await.unwrap(),
            "none"
        );
        // an explicit (source-configured) referrer survives the redirect unchanged
        let got = c
            .get(&url)
            .header("referer", "https://portal.example/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(got, "https://portal.example/");
    }
}
