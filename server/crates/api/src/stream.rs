//! Access control for the HLS stream routes served by `reel-playback`.
//!
//! The playback crate's router is unauthenticated; this middleware requires either the session
//! owner's bearer token (admins may watch any session) or a signature for scope `stream:{sid}`.
//! Signed playlists are rewritten so every relative URI (variant playlist, init segment, media
//! segments) carries the same signature, since players drop the query string when resolving
//! relative URLs.

use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    auth::{SignedQuery, bearer},
    error::ApiError,
};

fn session_id(path: &str) -> Option<&str> {
    path.split('/')
        .skip_while(|s| *s != "stream")
        .nth(1)
        .filter(|s| !s.is_empty())
}

/// Append `?{q}` to every URI of an HLS playlist.
pub fn sign_playlist(text: &str, q: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:URI=\"") {
            match rest.split_once('"') {
                Some((uri, tail)) => out.push_str(&format!("#EXT-X-MAP:URI=\"{uri}?{q}\"{tail}")),
                None => out.push_str(line),
            }
        } else if line.is_empty() || line.starts_with('#') {
            out.push_str(line);
        } else {
            out.push_str(&format!("{line}?{q}"));
        }
        out.push('\n');
    }
    out
}

pub async fn stream_guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let Some(sid) = session_id(req.uri().path()).map(str::to_string) else {
        return ApiError::NotFound.into_response();
    };
    let sig = Query::<SignedQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();
    let scope = format!("stream:{sid}");
    let mut signed_query = None;
    if let Some(token) = bearer(req.headers()) {
        let user = match st.auth.authenticate(token).await {
            Ok((u, _)) => u,
            Err(e) => return ApiError::from(e).into_response(),
        };
        if let Ok(info) = st.playback.session(&sid).await {
            if info.user_id != user.id && !user.is_admin() {
                return ApiError::NotFound.into_response();
            }
        }
    } else if let (Some(exp), Some(u), Some(s)) = (sig.exp, sig.u, sig.sig.as_deref()) {
        if let Err(e) = st.auth.verify_signature(&scope, u, exp, s).await {
            return ApiError::from(e).into_response();
        }
        signed_query = Some(format!("exp={exp}&u={u}&sig={s}"));
    } else {
        return ApiError::Unauthorized("authentication required (token or signed URL)".into())
            .into_response();
    }

    let is_playlist = req.uri().path().ends_with(".m3u8");
    let res = next.run(req).await;
    match signed_query {
        Some(q) if is_playlist && res.status() == StatusCode::OK => {
            let (mut parts, body) = res.into_parts();
            let Ok(bytes) = axum::body::to_bytes(body, 4 * 1024 * 1024).await else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            let text = sign_playlist(&String::from_utf8_lossy(&bytes), &q);
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(text))
        }
        _ => res,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_uris_get_signature() {
        let src = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.000,\nseg_00000.m4s\nmain/index.m3u8\n#EXT-X-ENDLIST\n";
        let out = sign_playlist(src, "exp=1&u=2&sig=x");
        assert!(out.contains("#EXT-X-MAP:URI=\"init.mp4?exp=1&u=2&sig=x\""));
        assert!(out.contains("\nseg_00000.m4s?exp=1&u=2&sig=x\n"));
        assert!(out.contains("\nmain/index.m3u8?exp=1&u=2&sig=x\n"));
        assert!(out.contains("#EXTINF:4.000,\n"));
        assert_eq!(session_id("/stream/abc/master.m3u8"), Some("abc"));
        assert_eq!(
            session_id("/api/v1/stream/abc/main/index.m3u8"),
            Some("abc")
        );
    }
}
