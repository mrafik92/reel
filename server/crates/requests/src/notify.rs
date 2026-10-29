//! Notification senders: Discord webhook, Telegram bot, generic JSON webhook.
//!
//! `notification_targets.config_json`:
//! - discord: `{"webhook_url": "..."}`
//! - telegram: `{"bot_token": "...", "chat_id": "...", "api_base": "https://api.telegram.org"?}`
//! - webhook: `{"url": "...", "headers": {"Authorization": "..."}?}`
//!
//! `events_json`: array of event names; empty = all events.

use std::{sync::Arc, time::Duration};

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{Error, Result};

/// Notification event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    Requested,
    Approved,
    Declined,
    Available,
    Failed,
    Issue,
}

impl NotifyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyKind::Requested => "requested",
            NotifyKind::Approved => "approved",
            NotifyKind::Declined => "declined",
            NotifyKind::Available => "available",
            NotifyKind::Failed => "failed",
            NotifyKind::Issue => "issue",
        }
    }
    fn color(self) -> u32 {
        match self {
            NotifyKind::Requested => 0x3b82f6,
            NotifyKind::Approved => 0x8b5cf6,
            NotifyKind::Declined | NotifyKind::Failed => 0xef4444,
            NotifyKind::Available => 0x22c55e,
            NotifyKind::Issue => 0xf59e0b,
        }
    }
}

/// A notification to deliver.
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub event: NotifyKind,
    pub request_id: Option<i64>,
    pub title: String,
    pub media_type: Option<String>,
    pub tmdb_id: Option<i64>,
    pub user: Option<String>,
    pub message: Option<String>,
}

impl Notification {
    /// Human-readable one-liner.
    pub fn text(&self) -> String {
        let verb = match self.event {
            NotifyKind::Requested => "was requested",
            NotifyKind::Approved => "was approved",
            NotifyKind::Declined => "was declined",
            NotifyKind::Available => "is now available",
            NotifyKind::Failed => "failed to download",
            NotifyKind::Issue => "has a reported issue",
        };
        let mut s = format!("{} {}", self.title, verb);
        if let Some(u) = &self.user {
            s.push_str(&format!(" (by {u})"));
        }
        if let Some(m) = &self.message {
            s.push_str(&format!(" — {m}"));
        }
        s
    }
}

/// Retry policy for deliveries.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_secs(2),
        }
    }
}

/// A `notification_targets` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Target {
    pub id: i64,
    pub kind: String,
    pub config_json: String,
    pub events_json: String,
}

impl Target {
    fn wants(&self, k: NotifyKind) -> bool {
        match serde_json::from_str::<Vec<String>>(&self.events_json) {
            Ok(v) if !v.is_empty() => v.iter().any(|e| e == k.as_str()),
            _ => true,
        }
    }
}

/// Sends notifications to all configured targets.
#[derive(Clone)]
pub struct Notifier {
    pool: SqlitePool,
    http: reqwest::Client,
    retry: Arc<RetryPolicy>,
}

impl Notifier {
    pub fn new(pool: SqlitePool, retry: RetryPolicy) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self {
            pool,
            http,
            retry: Arc::new(retry),
        }
    }

    /// Fire-and-forget: spawns a task, never blocks the caller.
    pub fn send(&self, n: Notification) {
        let me = self.clone();
        tokio::spawn(async move {
            if let Err(e) = me.send_all(&n).await {
                tracing::warn!(error = %e, "notification dispatch failed");
            }
        });
    }

    /// Deliver to every matching target (awaits completion; each with retry).
    pub async fn send_all(&self, n: &Notification) -> Result<()> {
        let targets: Vec<Target> = sqlx::query_as(
            "SELECT id, kind, config_json, events_json FROM notification_targets ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut set = Vec::new();
        for t in targets.into_iter().filter(|t| t.wants(n.event)) {
            let me = self.clone();
            let n = n.clone();
            set.push(tokio::spawn(async move {
                if let Err(e) = me.deliver_with_retry(&t, &n).await {
                    tracing::warn!(target_id = t.id, kind = %t.kind, error = %e, "notification delivery failed");
                }
            }));
        }
        for h in set {
            let _ = h.await;
        }
        Ok(())
    }

    /// Deliver to one target with retries.
    pub async fn deliver_with_retry(&self, t: &Target, n: &Notification) -> Result<()> {
        let mut delay = self.retry.base_delay;
        let mut last = None;
        for attempt in 0..self.retry.attempts.max(1) {
            if attempt > 0 {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            match self.deliver(t, n).await {
                Ok(()) => return Ok(()),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("at least one attempt"))
    }

    /// Single delivery attempt.
    pub async fn deliver(&self, t: &Target, n: &Notification) -> Result<()> {
        let cfg: Value = serde_json::from_str(&t.config_json).unwrap_or(Value::Null);
        let s = |k: &str| cfg.get(k).and_then(Value::as_str).map(str::to_string);
        let req = match t.kind.as_str() {
            "discord" => {
                let url = s("webhook_url")
                    .ok_or_else(|| Error::NotConfigured("discord webhook_url missing".into()))?;
                self.http.post(url).json(&discord_payload(n))
            }
            "telegram" => {
                let token = s("bot_token")
                    .ok_or_else(|| Error::NotConfigured("telegram bot_token missing".into()))?;
                let chat = cfg
                    .get("chat_id")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .ok_or_else(|| Error::NotConfigured("telegram chat_id missing".into()))?;
                let base = s("api_base").unwrap_or_else(|| "https://api.telegram.org".into());
                self.http
                    .post(format!(
                        "{}/bot{}/sendMessage",
                        base.trim_end_matches('/'),
                        token
                    ))
                    .json(&json!({ "chat_id": chat, "text": n.text() }))
            }
            "webhook" => {
                let url =
                    s("url").ok_or_else(|| Error::NotConfigured("webhook url missing".into()))?;
                let mut r = self.http.post(url).json(&webhook_payload(n));
                if let Some(h) = cfg.get("headers").and_then(Value::as_object) {
                    for (k, v) in h {
                        if let Some(v) = v.as_str() {
                            r = r.header(k, v);
                        }
                    }
                }
                r
            }
            other => {
                return Err(Error::NotConfigured(format!(
                    "unsupported notification kind {other}"
                )));
            }
        };
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Remote {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(())
    }
}

/// Discord webhook body.
pub fn discord_payload(n: &Notification) -> Value {
    let mut fields = vec![];
    if let Some(u) = &n.user {
        fields.push(json!({"name":"Requested by","value":u,"inline":true}));
    }
    if let Some(m) = &n.media_type {
        fields.push(json!({"name":"Type","value":m,"inline":true}));
    }
    json!({
        "embeds": [{
            "title": n.title,
            "description": n.text(),
            "color": n.event.color(),
            "fields": fields,
        }]
    })
}

/// Generic webhook body.
pub fn webhook_payload(n: &Notification) -> Value {
    json!({
        "event": n.event.as_str(),
        "request_id": n.request_id,
        "title": n.title,
        "media_type": n.media_type,
        "tmdb_id": n.tmdb_id,
        "user": n.user,
        "message": n.message,
        "text": n.text(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, method, path},
    };

    fn note() -> Notification {
        Notification {
            event: NotifyKind::Available,
            request_id: Some(3),
            title: "Dune".into(),
            media_type: Some("movie".into()),
            tmdb_id: Some(438631),
            user: Some("rafik".into()),
            message: None,
        }
    }

    async fn setup(kind: &str, cfg: Value, events: &str) -> Notifier {
        let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
        sqlx::query(
            "INSERT INTO notification_targets (kind, config_json, events_json) VALUES (?,?,?)",
        )
        .bind(kind)
        .bind(cfg.to_string())
        .bind(events)
        .execute(&pool)
        .await
        .unwrap();
        Notifier::new(
            pool,
            RetryPolicy {
                attempts: 3,
                base_delay: Duration::from_millis(5),
            },
        )
    }

    #[tokio::test]
    async fn discord_retries_then_succeeds() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(2)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .and(body_partial_json(json!({"embeds":[{"title":"Dune"}]})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let n = setup(
            "discord",
            json!({"webhook_url": format!("{}/hook", s.uri())}),
            "[]",
        )
        .await;
        n.send_all(&note()).await.unwrap();
    }

    #[tokio::test]
    async fn telegram_and_generic_and_event_filter() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botTOK/sendMessage"))
            .and(body_partial_json(json!({"chat_id":"42"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
            .expect(1)
            .mount(&s)
            .await;
        let n = setup(
            "telegram",
            json!({"bot_token":"TOK","chat_id":"42","api_base": s.uri()}),
            r#"["available"]"#,
        )
        .await;
        n.send_all(&note()).await.unwrap();
        // filtered-out event: no further request (expect(1) above)
        let mut other = note();
        other.event = NotifyKind::Declined;
        n.send_all(&other).await.unwrap();

        let g = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/w"))
            .and(wiremock::matchers::header("authorization", "Bearer x"))
            .and(body_partial_json(
                json!({"event":"available","tmdb_id":438631}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&g)
            .await;
        let n = setup(
            "webhook",
            json!({"url": format!("{}/w", g.uri()), "headers":{"Authorization":"Bearer x"}}),
            "[]",
        )
        .await;
        n.send_all(&note()).await.unwrap();
    }
}
