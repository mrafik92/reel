//! WebSocket access: single-use short-lived tickets (so the long-lived session token never has to
//! appear in a URL) and a per-user socket cap.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{AppState, auth::AuthUser, error::ApiResult};

const TICKET_TTL: Duration = Duration::from_secs(60);
/// Sockets one user may hold open at once.
pub const MAX_SOCKETS_PER_USER: usize = 8;

#[derive(Default)]
pub struct WsHub {
    /// ticket → (user id, auth session id, issued)
    tickets: Mutex<HashMap<String, (i64, i64, Instant)>>,
    sockets: Mutex<HashMap<i64, usize>>,
}

/// Holds one of a user's socket slots until dropped.
pub struct SocketSlot {
    hub: Arc<WsHub>,
    user: i64,
}

impl Drop for SocketSlot {
    fn drop(&mut self) {
        let mut m = self.hub.sockets.lock().unwrap();
        if let Some(n) = m.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.user);
            }
        }
    }
}

impl WsHub {
    pub fn issue(&self, user: i64, session: i64) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use rand::RngCore;
        let mut b = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut b);
        let t = URL_SAFE_NO_PAD.encode(b);
        let mut m = self.tickets.lock().unwrap();
        let now = Instant::now();
        m.retain(|_, (_, _, at)| now.duration_since(*at) < TICKET_TTL);
        if m.len() > 10_000 {
            m.clear();
        }
        m.insert(t.clone(), (user, session, now));
        t
    }

    /// Consume a ticket (single use): `(user id, session id)`.
    pub fn redeem(&self, ticket: &str) -> Option<(i64, i64)> {
        let (u, s, at) = self.tickets.lock().unwrap().remove(ticket)?;
        (at.elapsed() < TICKET_TTL).then_some((u, s))
    }

    pub fn acquire(self: &Arc<Self>, user: i64) -> Option<SocketSlot> {
        let mut m = self.sockets.lock().unwrap();
        let n = m.entry(user).or_insert(0);
        if *n >= MAX_SOCKETS_PER_USER {
            return None;
        }
        *n += 1;
        Some(SocketSlot {
            hub: self.clone(),
            user,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WsTicket {
    /// Pass as `/events?ticket=…` within `expires_in_s`; valid once.
    pub ticket: String,
    pub expires_in_s: u64,
}

/// A 60-second, single-use ticket for opening the `/events` WebSocket without putting the session
/// token in the URL.
#[utoipa::path(post, path = "/auth/ws-ticket", responses((status = 200, body = WsTicket)))]
pub async fn ws_ticket(State(st): State<AppState>, u: AuthUser) -> ApiResult<Json<WsTicket>> {
    Ok(Json(WsTicket {
        ticket: st.ws.issue(u.id(), u.session_id),
        expires_in_s: TICKET_TTL.as_secs(),
    }))
}
