//! Xtream Codes `player_api.php` client.

use std::time::Duration;

use serde_json::Value;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct AccountInfo {
    pub status: Option<String>,
    pub active: bool,
    /// Unix seconds.
    pub exp_date: Option<i64>,
    pub max_connections: Option<u32>,
    pub active_connections: Option<u32>,
    pub is_trial: bool,
    pub server_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveStream {
    pub stream_id: i64,
    pub name: String,
    pub num: Option<i64>,
    pub icon: Option<String>,
    pub epg_channel_id: Option<String>,
    pub category_id: Option<String>,
    pub archive_days: Option<u32>,
}

#[derive(Clone)]
pub struct XtreamClient {
    http: reqwest::Client,
    base: String,
    user: String,
    pass: String,
    ua: Option<String>,
}

fn flex_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flex_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl XtreamClient {
    /// `base` may be given with or without a trailing `/` or `/player_api.php`.
    pub fn new(http: reqwest::Client, base: &str, user: &str, pass: &str) -> Self {
        let b = base.trim().trim_end_matches('/');
        let b = b.strip_suffix("/player_api.php").unwrap_or(b);
        Self {
            http,
            base: b.to_string(),
            user: user.to_string(),
            pass: pass.to_string(),
            ua: None,
        }
    }

    /// User-Agent for the API calls (default: a VLC-like player; providers often reject unknown ones).
    pub fn with_user_agent(mut self, ua: Option<String>) -> Self {
        self.ua = ua.filter(|u| !u.trim().is_empty());
        self
    }

    fn api(&self, action: Option<&str>) -> String {
        let mut u = format!(
            "{}/player_api.php?username={}&password={}",
            self.base,
            urlenc(&self.user),
            urlenc(&self.pass)
        );
        if let Some(a) = action {
            u.push_str("&action=");
            u.push_str(a);
        }
        u
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self
            .http
            .get(url)
            .header(
                "User-Agent",
                self.ua.as_deref().unwrap_or(crate::net::DEFAULT_UA),
            )
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Unavailable(format!(
                "provider answered HTTP {}",
                resp.status().as_u16()
            )));
        }
        let body = resp.bytes().await?;
        serde_json::from_slice(&body).map_err(|e| Error::Parse(format!("not JSON: {e}")))
    }

    pub async fn account_info(&self) -> Result<AccountInfo> {
        let v = self.get_json(&self.api(None)).await?;
        parse_account(&v)
    }

    pub async fn live_categories(&self) -> Result<Vec<(String, String)>> {
        let v = self
            .get_json(&self.api(Some("get_live_categories")))
            .await?;
        Ok(parse_categories(&v))
    }

    pub async fn live_streams(&self) -> Result<Vec<LiveStream>> {
        let v = self.get_json(&self.api(Some("get_live_streams"))).await?;
        Ok(parse_streams(&v))
    }

    /// Canonical live URL (`/live/{user}/{pass}/{id}.ts`).
    pub fn stream_url(&self, stream_id: i64) -> String {
        format!(
            "{}/live/{}/{}/{}.ts",
            self.base,
            urlenc(&self.user),
            urlenc(&self.pass),
            stream_id
        )
    }

    pub fn xmltv_url(&self) -> String {
        format!(
            "{}/xmltv.php?username={}&password={}",
            self.base,
            urlenc(&self.user),
            urlenc(&self.pass)
        )
    }
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn parse_account(v: &Value) -> Result<AccountInfo> {
    let u = v
        .get("user_info")
        .ok_or_else(|| Error::Parse("no user_info (wrong credentials?)".into()))?;
    let status = u.get("status").and_then(flex_str);
    let auth = u.get("auth").and_then(flex_i64).unwrap_or(1);
    Ok(AccountInfo {
        active: auth != 0
            && status
                .as_deref()
                .is_none_or(|s| s.eq_ignore_ascii_case("active")),
        status,
        exp_date: u.get("exp_date").and_then(flex_i64),
        max_connections: u
            .get("max_connections")
            .and_then(flex_i64)
            .map(|n| n.max(0) as u32),
        active_connections: u
            .get("active_cons")
            .and_then(flex_i64)
            .map(|n| n.max(0) as u32),
        is_trial: u.get("is_trial").and_then(flex_i64).unwrap_or(0) != 0,
        server_url: v.pointer("/server_info/url").and_then(flex_str),
    })
}

pub fn parse_categories(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some((
                flex_str(c.get("category_id")?)?,
                flex_str(c.get("category_name")?)?,
            ))
        })
        .collect()
}

pub fn parse_streams(v: &Value) -> Vec<LiveStream> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(LiveStream {
                stream_id: flex_i64(s.get("stream_id")?)?,
                name: flex_str(s.get("name")?)?,
                num: s.get("num").and_then(flex_i64),
                icon: s.get("stream_icon").and_then(flex_str),
                epg_channel_id: s.get("epg_channel_id").and_then(flex_str),
                category_id: s.get("category_id").and_then(flex_str),
                archive_days: (s.get("tv_archive").and_then(flex_i64).unwrap_or(0) != 0).then(
                    || {
                        s.get("tv_archive_duration")
                            .and_then(flex_i64)
                            .unwrap_or(0)
                            .max(0) as u32
                    },
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn account_with_stringly_typed_numbers() {
        let v = json!({"user_info":{"auth":1,"status":"Active","exp_date":"1893456000",
            "max_connections":"2","active_cons":"1","is_trial":"0"},
            "server_info":{"url":"provider.tv"}});
        let a = parse_account(&v).unwrap();
        assert!(a.active);
        assert_eq!(a.max_connections, Some(2));
        assert_eq!(a.active_connections, Some(1));
        assert_eq!(a.exp_date, Some(1_893_456_000));
        assert!(!a.is_trial);
        assert!(parse_account(&json!({"foo":1})).is_err());
        let expired = parse_account(&json!({"user_info":{"auth":1,"status":"Expired"}})).unwrap();
        assert!(!expired.active);
    }

    #[test]
    fn streams_and_categories() {
        let v = json!([
            {"num":1,"name":"CNN HD","stream_id":"101","stream_icon":"http://i/cnn.png",
             "epg_channel_id":"cnn.us","category_id":"5","tv_archive":1,"tv_archive_duration":"3"},
            {"num":2,"name":"No id","stream_id":null},
            {"num":3,"name":"Radio","stream_id":102,"epg_channel_id":"","category_id":null,"tv_archive":0}
        ]);
        let s = parse_streams(&v);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].stream_id, 101);
        assert_eq!(s[0].epg_channel_id.as_deref(), Some("cnn.us"));
        assert_eq!(s[0].archive_days, Some(3));
        assert_eq!(s[1].epg_channel_id, None);
        assert_eq!(s[1].archive_days, None);
        assert_eq!(
            parse_categories(
                &json!([{"category_id":"5","category_name":"News"},{"category_id":6}])
            ),
            vec![("5".to_string(), "News".to_string())]
        );
    }

    #[test]
    fn urls() {
        let c = XtreamClient::new(
            reqwest::Client::new(),
            "http://h:8080/player_api.php",
            "us er",
            "p@ss",
        );
        assert_eq!(c.stream_url(7), "http://h:8080/live/us%20er/p%40ss/7.ts");
        assert_eq!(
            c.xmltv_url(),
            "http://h:8080/xmltv.php?username=us%20er&password=p%40ss"
        );
        assert!(
            c.api(Some("get_live_streams"))
                .ends_with("&action=get_live_streams")
        );
    }
}
