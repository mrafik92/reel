//! Playback routes: decide, progress, subtitles, admin sessions, watched toggles.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use reel_playback::{
    DecisionInput, DeviceProfile, HwAccelStatus, PlaybackDecision, PlaybackMode, Quality,
    QualityOption, QualityRequest, decide_quality,
    decision::is_image_subtitle,
    model::{self, StreamKind},
    subtitles::{SubtitleSource, to_vtt_with},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::{AdminUser, AuthUser, SignedQuery},
    dto::AudioStream,
    error::{ApiError, ApiResult, ErrorBody},
};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DecideRequest {
    /// Full device profile. Takes precedence over `preset`.
    pub profile: Option<DeviceProfile>,
    /// Built-in preset: web_chrome (default), web_safari, webos, tizen, android_exoplayer.
    pub preset: Option<String>,
    /// ffprobe stream index of the audio track to use. Omit to let the server choose from the
    /// user's preferences (`GET /me/preferences`).
    pub audio_index: Option<i64>,
    /// ffprobe stream index of the subtitle track to use. Omit for the automatic choice, `-1`
    /// for no subtitles.
    pub subtitle_index: Option<i64>,
    /// Start position in seconds (transcode sessions start there).
    pub start_s: Option<f64>,
    pub device_id: Option<String>,
    /// Multi-episode files: the episode being watched (one of the file's `item_ids`); watch
    /// progress is credited to it. Defaults to the file's first item.
    pub item_id: Option<i64>,
    /// Receiver is a Chromecast/AirPlay device: defaults the preset to `chromecast` and returns
    /// absolute, 8-hour signed URLs on `cast_base_url`.
    pub for_cast: Option<bool>,
    /// `auto` (default) | `original` | `1080p` | `720p` | `480p`. Auto keeps the original streams
    /// when they fit the bandwidth cap, else picks the best H.264 ladder rung that does.
    pub quality: Option<Quality>,
    /// Client bandwidth estimate (kbit/s). The effective cap is `min(this, your bitrate limit)`.
    pub max_bitrate_kbps: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubtitleOption {
    pub index: i64,
    pub stream_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// WebVTT URL; absent for image subtitles (PGS/VobSub) which can only be burned in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub forced: bool,
    pub external: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DecideResponse {
    pub session_id: String,
    pub mode: PlaybackMode,
    /// `/api/v1/direct/{id}` for direct play, else `/api/v1/stream/{session}/master.m3u8`.
    pub url: String,
    pub decision: PlaybackDecision,
    pub subtitles: Vec<SubtitleOption>,
    pub audio: Vec<AudioStream>,
    /// The tracks chosen automatically (or taken from the request) and why.
    pub selected: reel_playback::Selected,
    /// Expected bitrate (kbit/s) of the chosen output (source bitrate for direct play/remux).
    pub bitrate_kbps: u32,
    /// Resolved quality: `original` or a ladder rung (never `auto`).
    pub quality: Quality,
    /// Choices for a quality menu, with expected bitrates and availability (never upscales,
    /// respects the user's bitrate limit).
    pub quality_options: Vec<QualityOption>,
}

/// Re-deciding for the same file on the same device (quality switch, resume position) replaces
/// the previous session: it neither counts against stream/transcode limits nor lingers.
pub(crate) async fn replace_previous(
    st: &AppState,
    user_id: i64,
    device_id: &Option<String>,
    file_id: i64,
) {
    // without a device id we cannot tell a re-decide from a second player: leave those alone
    if device_id.is_none() {
        return;
    }
    for s in st.playback.list().await {
        if s.user_id == user_id && s.file_id == file_id && s.device_id == *device_id {
            st.end_session(&s.id).await;
        }
    }
}

/// Decide how to play a file and start a playback session.
#[utoipa::path(
    post,
    path = "/playback/{file_id}/decide",
    params(("file_id" = i64, Path, description = "Media file id")),
    request_body = DecideRequest,
    responses(
        (status = 200, body = DecideResponse),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 429, description = "Transcode limit reached", body = ErrorBody)
    )
)]
pub async fn decide_playback(
    State(st): State<AppState>,
    u: AuthUser,
    Path(file_id): Path<i64>,
    Json(req): Json<DecideRequest>,
) -> ApiResult<Json<DecideResponse>> {
    let cast = crate::prefs::cast_ctx(&st, req.for_cast.unwrap_or(false))?;
    let profile = match (req.profile, req.preset.as_deref()) {
        (Some(p), _) => crate::prefs::sanitize_profile(&u.user, p),
        (None, name) => {
            let name = name.unwrap_or(if cast.is_some() {
                "chromecast"
            } else {
                "web_chrome"
            });
            DeviceProfile::preset(name).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "unknown preset '{name}' (available: {})",
                    DeviceProfile::preset_names().join(", ")
                ))
            })?
        }
    };
    let primary_item = st.require_file(&u.user, file_id).await?;
    let watched_item = match req.item_id {
        None => primary_item,
        Some(i) if i == primary_item => i,
        Some(i) => {
            let linked: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM media_file_links WHERE file_id = ? AND item_id = ?",
            )
            .bind(file_id)
            .bind(i)
            .fetch_optional(&st.db)
            .await?;
            linked
                .ok_or_else(|| ApiError::BadRequest("item_id is not part of this file".into()))?;
            i
        }
    };
    let (file, streams) = model::load_file(&st.db, file_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    replace_previous(&st, u.id(), &req.device_id, file_id).await;
    let limits = &u.user.limits;
    let max_bitrate_kbps = limits.max_bitrate_kbps;
    // Concurrent-stream limit (every live session of this user counts).
    let mine: Vec<_> = st
        .playback
        .list()
        .await
        .into_iter()
        .filter(|s| s.user_id == u.id())
        .collect();
    if let Some(max) = limits.max_streams {
        if mine.len() as i64 >= max {
            return Err(ApiError::TooManyRequests(format!(
                "You reached your limit of {max} concurrent stream(s); stop another stream first"
            )));
        }
    }
    // automatic audio/subtitle choice (client indexes override)
    let prefs = crate::prefs::load(&st, u.id()).await;
    let original = if crate::prefs::wants_original(&prefs) {
        crate::prefs::library_original_language(&st, primary_item).await
    } else {
        None
    };
    let selected = reel_playback::select::select(
        &streams,
        &prefs,
        original.as_deref(),
        reel_playback::Pick::from_request(req.audio_index),
        reel_playback::Pick::from_request(req.subtitle_index),
        &profile,
    );
    let qd = decide_quality(
        &DecisionInput {
            file: &file,
            streams: &streams,
            audio_index: selected.audio_index,
            subtitle_index: selected.subtitle_index,
            profile: &profile,
            max_bitrate_kbps: max_bitrate_kbps.and_then(|v| u32::try_from(v).ok()),
        },
        &QualityRequest {
            quality: req.quality.unwrap_or_default(),
            max_bitrate_kbps: req.max_bitrate_kbps.filter(|v| *v > 0),
        },
    );
    let (decision, quality, bitrate_kbps, quality_options) =
        (qd.decision, qd.quality, qd.bitrate_kbps, qd.options);
    if let Some(max) = limits.max_transcodes {
        let transcoding = mine
            .iter()
            .filter(|s| {
                matches!(
                    s.mode,
                    PlaybackMode::Transcode | PlaybackMode::AudioTranscode
                )
            })
            .count() as i64;
        let needs = matches!(
            decision.mode,
            PlaybackMode::Transcode | PlaybackMode::AudioTranscode
        );
        if needs && transcoding >= max {
            return Err(ApiError::TooManyRequests(format!(
                "You reached your limit of {max} concurrent transcoded stream(s); stop another stream first"
            )));
        }
    }
    let info = st
        .playback
        .create_session(reel_playback::StartRequest {
            user_id: u.id(),
            device_id: req.device_id.clone(),
            file: file.clone(),
            streams: streams.clone(),
            decision: decision.clone(),
            start_position_s: req.start_s.unwrap_or(0.0).max(0.0),
            user_max_streams: limits.max_streams.and_then(|v| usize::try_from(v).ok()),
            user_max_transcodes: limits.max_transcodes.and_then(|v| usize::try_from(v).ok()),
            user_agent: None,
        })
        .await?;
    let sg = st.signer(u.id());
    let ttl = if cast.is_some() {
        crate::prefs::Cast::ttl(&st)
    } else {
        st.media_ttl()
    };
    let absolute = |u: String| match &cast {
        Some(c) => c.abs(&u),
        None => u,
    };
    let url = absolute(if decision.mode == PlaybackMode::DirectPlay {
        // `s` lets media requests (e.g. from a cast receiver) keep the session alive
        sg.url(
            &if cast.is_some() {
                format!("/api/v1/direct/{file_id}?s={}", info.id)
            } else {
                format!("/api/v1/direct/{file_id}")
            },
            &format!("direct:{file_id}"),
            ttl,
            1,
        )
    } else {
        sg.url(
            &format!("/api/v1/stream/{}/master.m3u8", info.id),
            &format!("stream:{}", info.id),
            ttl,
            1,
        )
    });
    let row = sqlx::query(
        "INSERT INTO playback_sessions (user_id, device_id, file_id, mode, video_decision_json, \
         bitrate, session_key, item_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(u.id())
    .bind(&req.device_id)
    .bind(file_id)
    .bind(decision.mode.db_mode())
    .bind(serde_json::to_string(&decision).ok())
    .bind(info.bitrate_kbps.map(i64::from))
    .bind(&info.id)
    .bind(watched_item)
    .execute(&st.db)
    .await;
    if let Err(e) = row {
        tracing::warn!("cannot persist playback session: {e}");
    }

    let subtitles = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .map(|s| SubtitleOption {
            index: s.idx,
            stream_id: s.id,
            language: s.language.clone(),
            title: s.title.clone(),
            url: (!is_image_subtitle(s.codec.as_deref().unwrap_or(""))).then(|| {
                absolute(sg.url(
                    &format!("/api/v1/subs/{}.vtt", s.id),
                    &format!("subs:{}", s.id),
                    ttl,
                    1,
                ))
            }),
            forced: s.forced,
            external: s.external_path.is_some(),
        })
        .collect();
    let audio = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .map(|s| AudioStream {
            index: s.idx,
            codec: s.codec.clone().unwrap_or_default(),
            channels: s.channels.unwrap_or(0),
            language: s.language.clone(),
            title: s.title.clone(),
            default: s.default,
        })
        .collect();
    Ok(Json(DecideResponse {
        session_id: info.id,
        mode: decision.mode,
        url,
        decision,
        subtitles,
        audio,
        selected,
        bitrate_kbps,
        quality,
        quality_options,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProgressRequest {
    pub position_s: f64,
    #[serde(default)]
    #[allow(dead_code)] // accepted for clients; pause state is not tracked yet
    pub paused: bool,
}

/// Report playback position; keeps the session alive and updates watch state.
#[utoipa::path(
    post,
    path = "/playback/{session_id}/progress",
    params(("session_id" = String, Path, description = "Session id from decide")),
    request_body = ProgressRequest,
    responses((status = 204, description = "Recorded"), (status = 404, body = ErrorBody))
)]
pub async fn playback_progress(
    State(st): State<AppState>,
    u: AuthUser,
    Path(sid): Path<String>,
    Json(req): Json<ProgressRequest>,
) -> ApiResult<StatusCode> {
    let info = st.playback.session(&sid).await?;
    if info.user_id != u.id() {
        return Err(ApiError::NotFound);
    }
    let pos = req.position_s.max(0.0);
    st.playback.update_progress(&sid, pos, None)?;
    // VOD sessions keep their own watch state
    if let Some(vs) = st.vod.get(&sid) {
        crate::vod::record_progress(&st, &vs, pos).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let primary: i64 = sqlx::query_scalar("SELECT item_id FROM media_files WHERE id = ?")
        .bind(info.file_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Episode 2+ of a multi-episode file: progress belongs to the episode the session was started for.
    let item_id: i64 =
        sqlx::query_scalar("SELECT item_id FROM playback_sessions WHERE session_key = ?")
            .bind(&sid)
            .fetch_optional(&st.db)
            .await?
            .flatten()
            .unwrap_or(primary);
    let duration: Option<f64> =
        sqlx::query_scalar("SELECT duration_s FROM media_files WHERE id = ?")
            .bind(info.file_id)
            .fetch_one(&st.db)
            .await?;
    let finished = duration.is_some_and(|d| d > 0.0 && pos >= d * 0.9);
    let ts = now();
    // `played` is sticky: re-watching never un-marks an item. Finishing a multi-episode file
    // marks every episode in it played.
    let upsert = "INSERT INTO watch_state (user_id, item_id, position_s, played, play_count, last_played_at) \
         VALUES (?1, ?2, ?3, ?4, ?4, ?5) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
           position_s = excluded.position_s, \
           play_count = play_count + (CASE WHEN excluded.played = 1 AND played = 0 THEN 1 ELSE 0 END), \
           played = MAX(played, excluded.played), \
           last_played_at = excluded.last_played_at";
    sqlx::query(upsert)
        .bind(info.user_id)
        .bind(item_id)
        .bind(pos)
        .bind(i64::from(finished))
        .bind(ts)
        .execute(&st.db)
        .await?;
    if finished {
        let others: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM media_file_links WHERE file_id = ?1 UNION \
             SELECT item_id FROM media_files WHERE id = ?1",
        )
        .bind(info.file_id)
        .fetch_all(&st.db)
        .await?;
        for o in others.into_iter().filter(|o| *o != item_id) {
            sqlx::query(upsert)
                .bind(info.user_id)
                .bind(o)
                .bind(0.0_f64)
                .bind(1_i64)
                .bind(ts)
                .execute(&st.db)
                .await?;
        }
    }
    let _ = sqlx::query("UPDATE playback_sessions SET last_ping = ? WHERE session_key = ?")
        .bind(ts)
        .bind(&sid)
        .execute(&st.db)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Stop a playback session (kills ffmpeg, removes temp files).
#[utoipa::path(
    delete,
    path = "/playback/{session_id}",
    params(("session_id" = String, Path, description = "Session id from decide")),
    responses((status = 204, description = "Stopped"), (status = 404, body = ErrorBody))
)]
pub async fn stop_playback(
    State(st): State<AppState>,
    u: AuthUser,
    Path(sid): Path<String>,
) -> ApiResult<StatusCode> {
    let info = st.playback.session(&sid).await?;
    if info.user_id != u.id() && !u.is_admin() {
        return Err(ApiError::NotFound);
    }
    st.end_session(&sid)
        .await
        .then_some(())
        .ok_or(ApiError::NotFound)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Subtitle track converted to WebVTT (cached).
#[utoipa::path(
    get,
    path = "/subs/{file}",
    params(("file" = String, Path, description = "`{stream_id}.vtt`"), SignedQuery),
    responses(
        (status = 200, description = "WebVTT", content_type = "text/vtt"),
        (status = 404, body = ErrorBody),
        (status = 400, description = "Image subtitle cannot be converted", body = ErrorBody)
    )
)]
pub async fn subtitle_vtt(
    State(st): State<AppState>,
    Path(file): Path<String>,
    Query(sq): Query<SignedQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let stream_id: i64 = file
        .strip_suffix(".vtt")
        .and_then(|n| n.parse().ok())
        .ok_or(ApiError::NotFound)?;
    let user = st
        .media_user(&headers, &sq, &format!("subs:{stream_id}"))
        .await?;
    let file_id: i64 = sqlx::query_scalar("SELECT file_id FROM streams WHERE id = ?")
        .bind(stream_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    st.require_file(&user, file_id).await?;
    let row = sqlx::query(
        "SELECT s.idx, s.kind, s.codec, s.external_path, f.path FROM streams s \
         JOIN media_files f ON f.id = s.file_id WHERE s.id = ?",
    )
    .bind(stream_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    if row.get::<String, _>("kind") != "subtitle" {
        return Err(ApiError::NotFound);
    }
    if is_image_subtitle(
        row.get::<Option<String>, _>("codec")
            .as_deref()
            .unwrap_or(""),
    ) {
        return Err(ApiError::BadRequest(
            "image subtitles cannot be converted to WebVTT".into(),
        ));
    }
    let src = match row.get::<Option<String>, _>("external_path") {
        Some(path) => SubtitleSource::External { path },
        None => SubtitleSource::Embedded {
            path: row.get("path"),
            stream_index: row.get("idx"),
        },
    };
    let out = to_vtt_with(
        st.playback.media(),
        &st.playback.config().ffmpeg_path,
        &src,
        &st.config.data_dir.join("subs"),
        &format!("stream-{stream_id}"),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let bytes = tokio::fs::read(out)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut res = bytes.into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/vtt; charset=utf-8"),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    Ok(res)
}

// ───────────────────────── admin ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminSession {
    pub id: String,
    pub user_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub file_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_title: Option<String>,
    pub mode: PlaybackMode,
    pub reasons: Vec<String>,
    pub decision: PlaybackDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub position_s: f64,
    /// 0..1, when the file duration is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    pub hwaccel: String,
    /// ffmpeg is currently running.
    pub active: bool,
    pub started_at: i64,
    pub last_ping: i64,
}

/// Live playback sessions.
#[utoipa::path(get, path = "/admin/sessions", responses((status = 200, body = Vec<AdminSession>)))]
pub async fn admin_sessions(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<AdminSession>>> {
    let live = st.playback.list().await;
    let mut out = Vec::with_capacity(live.len());
    for s in live {
        let r = sqlx::query(
            "SELECT i.id AS item_id, i.title, f.duration_s, \
             (SELECT username FROM users WHERE id = ?) AS username \
             FROM media_files f JOIN media_items i ON i.id = f.item_id WHERE f.id = ?",
        )
        .bind(s.user_id)
        .bind(s.file_id)
        .fetch_optional(&st.db)
        .await?;
        let dur: Option<f64> = r.as_ref().and_then(|r| r.get("duration_s"));
        out.push(AdminSession {
            reasons: s.decision.reasons.clone(),
            progress: dur
                .filter(|d| *d > 0.0)
                .map(|d| (s.position_s / d).clamp(0.0, 1.0)),
            username: r.as_ref().and_then(|r| r.get("username")),
            item_id: r.as_ref().map(|r| r.get("item_id")),
            item_title: r.as_ref().map(|r| r.get("title")),
            id: s.id,
            user_id: s.user_id,
            file_id: s.file_id,
            mode: s.mode,
            decision: s.decision,
            device_id: s.device_id,
            position_s: s.position_s,
            bitrate_kbps: s.bitrate_kbps,
            hwaccel: s.hwaccel,
            active: s.active,
            started_at: s.started_at,
            last_ping: s.last_ping,
        });
    }
    // Drop DB rows of sessions that ended without an explicit DELETE (reaped).
    let keys: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, session_key FROM playback_sessions")
            .fetch_all(&st.db)
            .await?;
    for (id, key) in rows {
        if key.as_deref().is_none_or(|k| !keys.contains(&k)) {
            let _ = sqlx::query("DELETE FROM playback_sessions WHERE id = ?")
                .bind(id)
                .execute(&st.db)
                .await;
        }
    }
    Ok(Json(out))
}

/// Kill a playback session.
#[utoipa::path(
    delete,
    path = "/admin/sessions/{id}",
    params(("id" = String, Path, description = "Session id")),
    responses((status = 204, description = "Killed"), (status = 404, body = ErrorBody))
)]
pub async fn admin_kill_session(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(sid): Path<String>,
) -> ApiResult<StatusCode> {
    st.end_session(&sid)
        .await
        .then_some(())
        .ok_or(ApiError::NotFound)?;
    st.audit(&a.user, "session_kill", Some(sid), serde_json::Value::Null)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Hardware acceleration detected at startup.
#[utoipa::path(get, path = "/admin/hwaccel", responses((status = 200, body = HwAccelStatus)))]
pub async fn admin_hwaccel(State(st): State<AppState>, _a: AdminUser) -> Json<HwAccelStatus> {
    Json((*st.hwaccel).clone())
}

// ───────────────────────── watched toggles ─────────────────────────

/// Movie/episode ids at or below `item_id` (item itself, seasons' episodes, …).
async fn leaf_ids(st: &AppState, user: &reel_auth::User, item_id: i64) -> ApiResult<Vec<i64>> {
    st.require_item(user, item_id).await?;
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE t(id) AS (SELECT ?1 UNION ALL \
           SELECT m.id FROM media_items m JOIN t ON m.parent_id = t.id) \
         SELECT m.id FROM media_items m JOIN t ON t.id = m.id \
         WHERE m.kind IN ('movie','episode')",
    )
    .bind(item_id)
    .fetch_all(&st.db)
    .await?)
}

/// Mark an item played (cascades to episodes for seasons/shows).
#[utoipa::path(
    post,
    path = "/items/{id}/played",
    params(("id" = i64, Path, description = "Item id")),
    responses((status = 204, description = "Marked played"), (status = 404, body = ErrorBody))
)]
pub async fn mark_played(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let ids = leaf_ids(&st, &u.user, id).await?;
    let ts = now();
    let mut tx = st.db.begin_with("BEGIN IMMEDIATE").await?;
    for i in ids {
        sqlx::query(
            "INSERT INTO watch_state (user_id, item_id, position_s, played, play_count, last_played_at) \
             VALUES (?1, ?2, 0, 1, 1, ?3) \
             ON CONFLICT(user_id, item_id) DO UPDATE SET position_s = 0, \
               play_count = play_count + (CASE WHEN played = 0 THEN 1 ELSE 0 END), \
               played = 1, last_played_at = ?3",
        )
        .bind(u.id())
        .bind(i)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Mark an item unplayed and reset its progress (cascades like `POST`).
#[utoipa::path(
    delete,
    path = "/items/{id}/played",
    params(("id" = i64, Path, description = "Item id")),
    responses((status = 204, description = "Marked unplayed"), (status = 404, body = ErrorBody))
)]
pub async fn mark_unplayed(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let ids = leaf_ids(&st, &u.user, id).await?;
    let mut tx = st.db.begin_with("BEGIN IMMEDIATE").await?;
    for i in ids {
        sqlx::query("DELETE FROM watch_state WHERE user_id = ? AND item_id = ?")
            .bind(u.id())
            .bind(i)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────────── HLS stream routes (served by reel-playback; documented here) ─────────────

/// HLS master playlist of a transcode/remux session.
#[utoipa::path(
    get,
    path = "/stream/{session_id}/master.m3u8",
    params(("session_id" = String, Path, description = "Session id from decide"), SignedQuery),
    responses((status = 200, description = "HLS master playlist", content_type = "application/vnd.apple.mpegurl"), (status = 404))
)]
#[allow(dead_code)]
pub fn doc_stream_master() {}

/// HLS media playlist (`index.m3u8`) or fMP4 segment/init file of a session.
#[utoipa::path(
    get,
    path = "/stream/{session_id}/{variant}/{file}",
    params(
        ("session_id" = String, Path, description = "Session id"),
        ("variant" = String, Path, description = "Always `main`"),
        ("file" = String, Path, description = "`index.m3u8`, `init.mp4` or `seg_NNNNN.m4s`"),
        SignedQuery
    ),
    responses((status = 200, description = "Playlist or segment"), (status = 404), (status = 503, description = "Segment not ready"))
)]
#[allow(dead_code)]
pub fn doc_stream_file() {}
