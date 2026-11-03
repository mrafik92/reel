//! Per-user playback preferences, language-aware track selection helpers and cast URLs.

use axum::{
    Json,
    extract::{Path, State},
};
use reel_playback::{
    PlaybackMode, PlaybackPrefs, SubtitleMode,
    langs::normalize_lang,
    model::{self, StreamKind},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::AuthUser,
    error::{ApiError, ApiResult, ErrorBody},
};

// ───────────────────────── preferences ─────────────────────────

/// Stored preferences of a user (defaults when none were saved).
pub(crate) async fn load(st: &AppState, user_id: i64) -> PlaybackPrefs {
    let json: Option<String> =
        sqlx::query_scalar("SELECT json FROM user_preferences WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    json.and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn clean(list: &[String], what: &str, allow_original: bool) -> ApiResult<Vec<String>> {
    if list.len() > 10 {
        return Err(ApiError::BadRequest(format!(
            "{what}: at most 10 languages"
        )));
    }
    let mut out: Vec<String> = vec![];
    for raw in list {
        let code = if raw.trim().eq_ignore_ascii_case("original") {
            if !allow_original {
                return Err(ApiError::BadRequest(format!(
                    "{what}: 'original' is only valid for audio"
                )));
            }
            "original".to_string()
        } else {
            normalize_lang(raw).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "{what}: unknown language '{}'",
                    raw.chars().take(30).collect::<String>()
                ))
            })?
        };
        if !out.contains(&code) {
            out.push(code);
        }
    }
    Ok(out)
}

/// The signed-in user's playback preferences.
#[utoipa::path(get, path = "/me/preferences", responses((status = 200, body = PlaybackPrefs)))]
pub async fn get_preferences(
    State(st): State<AppState>,
    u: AuthUser,
) -> ApiResult<Json<PlaybackPrefs>> {
    Ok(Json(load(&st, u.id()).await))
}

/// Replace the preferences (missing fields fall back to the defaults). Language codes are
/// normalised to ISO 639-1 where one exists (`eng`/`English` → `en`).
#[utoipa::path(
    put,
    path = "/me/preferences",
    request_body = PlaybackPrefs,
    responses((status = 200, body = PlaybackPrefs), (status = 400, body = ErrorBody))
)]
pub async fn put_preferences(
    State(st): State<AppState>,
    u: AuthUser,
    Json(p): Json<PlaybackPrefs>,
) -> ApiResult<Json<PlaybackPrefs>> {
    let mut audio = clean(&p.audio_languages, "audio_languages", true)?;
    if audio.is_empty() {
        audio.push("original".into());
    }
    let saved = PlaybackPrefs {
        audio_languages: audio,
        subtitle_mode: p.subtitle_mode,
        subtitle_languages: clean(&p.subtitle_languages, "subtitle_languages", false)?,
        prefer_sdh: p.prefer_sdh,
        burn_in_image_subs: p.burn_in_image_subs,
    };
    sqlx::query(
        "INSERT INTO user_preferences (user_id, json) VALUES (?1, ?2) \
         ON CONFLICT(user_id) DO UPDATE SET json = ?2, updated_at = unixepoch()",
    )
    .bind(u.id())
    .bind(serde_json::to_string(&saved).unwrap_or_default())
    .execute(&st.db)
    .await?;
    Ok(Json(saved))
}

/// Original language of a library item (TMDB), looked up only when the user's audio preference
/// asks for `original` and TMDB is configured. Best effort: never fails playback.
pub(crate) async fn library_original_language(st: &AppState, item_id: i64) -> Option<String> {
    let t = st.tmdb.as_ref()?;
    let mut id = item_id;
    // episode → season → show
    for _ in 0..3 {
        let row = sqlx::query("SELECT kind, tmdb_id, parent_id FROM media_items WHERE id = ?")
            .bind(id)
            .fetch_optional(&st.db)
            .await
            .ok()??;
        let kind: String = row.get("kind");
        let tmdb: Option<i64> = row.get("tmdb_id");
        match (kind.as_str(), tmdb) {
            ("movie", Some(t_id)) => return original_language(t, false, t_id).await,
            ("show", Some(t_id)) => return original_language(t, true, t_id).await,
            _ => id = row.get::<Option<i64>, _>("parent_id")?,
        }
    }
    None
}

pub(crate) async fn original_language(
    t: &reel_metadata::TmdbClient,
    tv: bool,
    tmdb_id: i64,
) -> Option<String> {
    let fut = async {
        if tv {
            t.tv_cached(tmdb_id)
                .await
                .ok()
                .and_then(|d| d.original_language)
        } else {
            t.movie_cached(tmdb_id)
                .await
                .ok()
                .and_then(|d| d.original_language)
        }
    };
    tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .ok()
        .flatten()
}

pub(crate) fn wants_original(p: &PlaybackPrefs) -> bool {
    p.audio_languages
        .iter()
        .any(|l| l.eq_ignore_ascii_case("original"))
}

// ───────────────────────── device profiles ─────────────────────────

/// A client-supplied device profile, made safe for non-admins: bounded sizes, sane resolution /
/// bitrate, and always able to take H.264, so a transcode can never be steered into a (software)
/// HEVC/AV1 encode. Admins are trusted as they are. Unknown JSON fields are already ignored.
pub(crate) fn sanitize_profile(
    u: &reel_auth::User,
    mut p: reel_playback::DeviceProfile,
) -> reel_playback::DeviceProfile {
    if u.is_admin() {
        return p;
    }
    p.name = p.name.chars().take(64).collect();
    p.containers.truncate(12);
    p.video.truncate(12);
    p.audio.truncate(16);
    p.subtitles.text.truncate(8);
    p.subtitles.image.truncate(8);
    p.max_width = p.max_width.clamp(160, 7680);
    p.max_height = p.max_height.clamp(120, 4320);
    p.max_bitrate_kbps = p.max_bitrate_kbps.map(|b| b.clamp(100, 200_000));
    if p.video_support("h264").is_none() {
        // put H.264 (any level a client decodes) first-class: the encoder target is H.264
        p.video.insert(
            0,
            reel_playback::profile::VideoCodecSupport {
                codec: "h264".into(),
                max_profile: Some("high".into()),
                max_level: Some(41),
                max_bit_depth: 8,
                hdr: vec![],
            },
        );
    }
    p
}

// ───────────────────────── cast ─────────────────────────

/// Cast URLs live this long (a long film plus pauses).
pub(crate) const CAST_TTL_S: i64 = 8 * 3600;

/// Absolute-URL context for a Chromecast / AirPlay receiver.
#[derive(Clone)]
pub(crate) struct Cast {
    base: String,
}

impl Cast {
    pub fn ttl(st: &AppState) -> i64 {
        st.media_ttl().max(CAST_TTL_S)
    }
    pub fn abs(&self, path_and_query: &str) -> String {
        format!("{}{}", self.base, path_and_query)
    }
}

/// `Some` when `for_cast` is requested (400 if the server has no `cast_base_url`/`public_url`).
pub(crate) fn cast_ctx(st: &AppState, for_cast: bool) -> ApiResult<Option<Cast>> {
    if !for_cast {
        return Ok(None);
    }
    let base = st
        .config
        .cast_base_url
        .as_deref()
        .or(st.config.public_url.as_deref())
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| b.starts_with("http"))
        .ok_or_else(|| {
            ApiError::BadRequest(
                "cast_base_url is not configured on the server (e.g. http://192.168.1.10:8420)"
                    .into(),
            )
        })?;
    Ok(Some(Cast { base }))
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CastSubtitle {
    pub index: i64,
    /// Absolute WebVTT URL (absent for image subtitles or when extraction is unavailable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub forced: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CastUrl {
    pub session_id: String,
    pub mode: PlaybackMode,
    /// Absolute signed media URL on `cast_base_url` (HLS master playlist, or the direct/proxied file).
    pub url: String,
    pub content_type: String,
    pub base_url: String,
    pub expires_in_s: i64,
    pub subtitles: Vec<CastSubtitle>,
}

/// Absolute, long-lived (8 h) signed URLs of a running playback session for a Chromecast /
/// AirPlay receiver. Start the session with `preset: "chromecast"` (or `for_cast: true`, which
/// defaults to it) so the decision suits the receiver. The sender must keep pinging
/// `/playback/{id}/progress`; media requests from the receiver also keep the session alive.
#[utoipa::path(
    post,
    path = "/playback/{session_id}/cast-url",
    params(("session_id" = String, Path, description = "Session id from decide / vod play")),
    responses((status = 200, body = CastUrl), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn cast_url(
    State(st): State<AppState>,
    u: AuthUser,
    Path(sid): Path<String>,
) -> ApiResult<Json<CastUrl>> {
    let info = st.playback.session(&sid).await?;
    if info.user_id != u.id() && !u.is_admin() {
        return Err(ApiError::NotFound);
    }
    let cast = cast_ctx(&st, true)?.expect("for_cast");
    let sg = st.signer(info.user_id);
    let ttl = Cast::ttl(&st);
    st.playback.touch_session(&sid);
    let vod = st.vod.get(&sid);
    let (path, scope, content_type) = if info.mode == PlaybackMode::DirectPlay {
        match &vod {
            Some(v) => (
                format!("/api/v1/vod/direct/{sid}"),
                format!("vod:{sid}"),
                crate::routes::container_mime(&v.container).to_string(),
            ),
            None => {
                let container: Option<String> =
                    sqlx::query_scalar("SELECT container FROM media_files WHERE id = ?")
                        .bind(info.file_id)
                        .fetch_optional(&st.db)
                        .await?
                        .flatten();
                (
                    format!("/api/v1/direct/{}?s={sid}", info.file_id),
                    format!("direct:{}", info.file_id),
                    crate::routes::container_mime(container.as_deref().unwrap_or("")).to_string(),
                )
            }
        }
    } else {
        (
            format!("/api/v1/stream/{sid}/master.m3u8"),
            format!("stream:{sid}"),
            "application/vnd.apple.mpegurl".to_string(),
        )
    };
    let url = cast.abs(&sg.url(&path, &scope, ttl, 1));
    // text subtitles as absolute WebVTT URLs
    let mut subtitles = vec![];
    if let Some(v) = &vod {
        let streams = crate::vod::cached_streams(&st, v.kind, v.item_id).await;
        for s in streams.iter().filter(|s| s.kind == StreamKind::Subtitle) {
            let text =
                !reel_playback::decision::is_image_subtitle(s.codec.as_deref().unwrap_or(""));
            let can = text && crate::vod::sub_available(&st, v, s.idx).await;
            subtitles.push(CastSubtitle {
                index: s.idx,
                url: can.then(|| {
                    cast.abs(&sg.url(
                        &format!("/api/v1/vod/subs/{sid}/{}.vtt", s.idx),
                        &format!("vodsub:{sid}:{}", s.idx),
                        ttl,
                        1,
                    ))
                }),
                language: s.language.clone(),
                forced: s.forced,
            });
        }
    } else if let Some((_, streams)) = model::load_file(&st.db, info.file_id).await? {
        for s in streams.iter().filter(|s| s.kind == StreamKind::Subtitle) {
            let text =
                !reel_playback::decision::is_image_subtitle(s.codec.as_deref().unwrap_or(""));
            subtitles.push(CastSubtitle {
                index: s.idx,
                url: text.then(|| {
                    cast.abs(&sg.url(
                        &format!("/api/v1/subs/{}.vtt", s.id),
                        &format!("subs:{}", s.id),
                        ttl,
                        1,
                    ))
                }),
                language: s.language.clone(),
                forced: s.forced,
            });
        }
    }
    Ok(Json(CastUrl {
        session_id: sid,
        mode: info.mode,
        url,
        content_type,
        base_url: cast.base,
        expires_in_s: ttl,
        subtitles,
    }))
}

#[allow(dead_code)]
const _MODE: SubtitleMode = SubtitleMode::Auto;
