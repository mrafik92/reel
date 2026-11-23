//! IPTV routes: channels, EPG, tune/live sessions, favorites, logos, and admin management.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use reel_iptv::{
    HlsOptions, LiveMode,
    store::{self, ChannelFilter, ChannelPatch, EpgProgram, Source, SourceKind},
    xtream::XtreamClient,
};
use reel_playback::DeviceProfile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    auth::{AdminUser, AuthUser, SignedQuery},
    error::{ApiError, ApiResult, ErrorBody},
};

const LOGO_TTL: i64 = 2 * 86_400;
const LOGO_QUANTUM: i64 = 86_400;

fn now() -> i64 {
    reel_auth::signing::now_unix()
}

// ───────────────────────── DTOs ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct IptvChannel {
    pub id: i64,
    pub name: String,
    pub number: Option<i64>,
    pub group: Option<String>,
    /// Signed, cached logo URL (`/api/v1/iptv/logo/{id}?…`).
    pub logo_url: Option<String>,
    pub favorite: bool,
    /// An EPG mapping exists for this channel.
    pub has_epg: bool,
    /// The provider offers catch-up/archive for this channel.
    pub catchup: bool,
}

fn channel_dto(c: &store::ChannelRow, sg: &reel_auth::UrlSigner) -> IptvChannel {
    IptvChannel {
        id: c.id,
        name: c.name.clone(),
        number: c.number,
        group: c.group.clone(),
        logo_url: c
            .logo
            .as_deref()
            .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
            .map(|_| {
                sg.url(
                    &format!("/api/v1/iptv/logo/{}", c.id),
                    &format!("logo:{}", c.id),
                    LOGO_TTL,
                    LOGO_QUANTUM,
                )
            }),
        favorite: c.favorite,
        has_epg: c.epg_channel_id.is_some() || c.tvg_id.is_some(),
        catchup: c.has_catchup,
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ChannelsQuery {
    /// Only this group.
    pub group: Option<String>,
    /// Only the user's favorites.
    pub favorites: Option<bool>,
    /// Case-insensitive name search.
    pub q: Option<String>,
}

/// Enabled live channels ordered by channel number, then name.
#[utoipa::path(get, path = "/iptv/channels", params(ChannelsQuery), responses((status = 200, body = Vec<IptvChannel>)))]
pub async fn list_channels(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<ChannelsQuery>,
) -> ApiResult<Json<Vec<IptvChannel>>> {
    let rows = store::list_channels(
        &st.db,
        &ChannelFilter {
            group: q.group,
            q: q.q.filter(|s| !s.trim().is_empty()),
            user_id: Some(u.id()),
            favorites_only: q.favorites.unwrap_or(false),
            ..Default::default()
        },
    )
    .await?;
    let sg = st.signer(u.id());
    Ok(Json(rows.iter().map(|c| channel_dto(c, &sg)).collect()))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IptvGroup {
    pub name: String,
    pub count: i64,
}

/// Channel groups with counts.
#[utoipa::path(get, path = "/iptv/groups", responses((status = 200, body = Vec<IptvGroup>)))]
pub async fn list_groups(
    State(st): State<AppState>,
    _u: AuthUser,
) -> ApiResult<Json<Vec<IptvGroup>>> {
    Ok(Json(
        store::list_groups(&st.db)
            .await?
            .into_iter()
            .map(|(name, count)| IptvGroup { name, count })
            .collect(),
    ))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Program {
    /// Unix seconds.
    pub start: i64,
    pub stop: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Only with `detail=true` (and for `/iptv/now`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_num: Option<String>,
}

impl From<EpgProgram> for Program {
    fn from(p: EpgProgram) -> Self {
        Program {
            start: p.start,
            stop: p.stop,
            title: p.title,
            sub_title: p.sub_title,
            category: p.category,
            description: p.description,
            icon: p.icon,
            episode_num: p.episode_num,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EpgChannel {
    pub channel_id: i64,
    pub programs: Vec<Program>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EpgGrid {
    pub from: i64,
    pub to: i64,
    pub channels: Vec<EpgChannel>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct EpgQuery {
    /// Window start, unix seconds (default: 1 hour ago).
    pub from: Option<i64>,
    /// Window end, unix seconds (default: from + 6 h; max span 48 h).
    pub to: Option<i64>,
    /// Comma-separated channel ids (default: the first 200 channels).
    pub channels: Option<String>,
    /// Include descriptions, icons and episode numbers (default: compact).
    pub detail: Option<bool>,
}

fn parse_ids(s: &str) -> ApiResult<Vec<i64>> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            p.trim()
                .parse()
                .map_err(|_| ApiError::BadRequest(format!("bad channel id '{p}'")))
        })
        .collect()
}

async fn default_channel_ids(st: &AppState, cap: usize) -> ApiResult<Vec<i64>> {
    let mut v: Vec<i64> = store::list_channels(&st.db, &ChannelFilter::default())
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    v.truncate(cap);
    Ok(v)
}

/// Programme grid for a time window — compact by default (no descriptions).
#[utoipa::path(get, path = "/iptv/epg", params(EpgQuery), responses((status = 200, body = EpgGrid), (status = 400, body = ErrorBody)))]
pub async fn epg_grid(
    State(st): State<AppState>,
    _u: AuthUser,
    Query(q): Query<EpgQuery>,
) -> ApiResult<Json<EpgGrid>> {
    let from = q.from.unwrap_or_else(|| now() - 3600);
    let to = q.to.unwrap_or(from + 6 * 3600);
    if to <= from || to - from > 48 * 3600 {
        return Err(ApiError::BadRequest(
            "window must be 0 < to-from <= 48 h".into(),
        ));
    }
    let ids = match q.channels.as_deref() {
        Some(s) => parse_ids(s)?,
        None => default_channel_ids(&st, 200).await?,
    };
    if ids.len() > 500 {
        return Err(ApiError::BadRequest(
            "at most 500 channels per request".into(),
        ));
    }
    // Xtream providers without XMLTV: fetch the visible channels' guide lazily
    st.iptv
        .epg
        .ensure(
            &ids[..ids.len().min(40)],
            std::time::Duration::from_millis(1200),
        )
        .await;
    let mut map = store::epg_range(&st.db, &ids, from, to, q.detail.unwrap_or(false)).await?;
    let channels = ids
        .into_iter()
        .map(|id| EpgChannel {
            channel_id: id,
            programs: map
                .remove(&id)
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
        })
        .collect();
    Ok(Json(EpgGrid { from, to, channels }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NowNext {
    pub channel_id: i64,
    pub now: Option<Program>,
    pub next: Option<Program>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct NowQuery {
    /// Comma-separated channel ids (default: the first 500 channels).
    pub channels: Option<String>,
}

/// What is on now and next, per channel.
#[utoipa::path(get, path = "/iptv/now", params(NowQuery), responses((status = 200, body = Vec<NowNext>)))]
pub async fn now_next(
    State(st): State<AppState>,
    _u: AuthUser,
    Query(q): Query<NowQuery>,
) -> ApiResult<Json<Vec<NowNext>>> {
    let ids = match q.channels.as_deref() {
        Some(s) => parse_ids(s)?,
        None => default_channel_ids(&st, 500).await?,
    };
    st.iptv
        .epg
        .ensure(
            &ids[..ids.len().min(40)],
            std::time::Duration::from_millis(1200),
        )
        .await;
    let mut map = store::now_next(&st.db, &ids, now()).await?;
    Ok(Json(
        ids.into_iter()
            .map(|id| {
                let (n, x) = map.remove(&id).unwrap_or((None, None));
                NowNext {
                    channel_id: id,
                    now: n.map(Into::into),
                    next: x.map(Into::into),
                }
            })
            .collect(),
    ))
}

// ───────────────────────── favorites ─────────────────────────

/// Add a channel to the user's favorites.
#[utoipa::path(
    post,
    path = "/iptv/channels/{id}/favorite",
    params(("id" = i64, Path, description = "Channel id")),
    responses((status = 204, description = "Added"), (status = 404, body = ErrorBody))
)]
pub async fn add_favorite(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    store::set_favorite(&st.db, u.id(), id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove a channel from the user's favorites.
#[utoipa::path(
    delete,
    path = "/iptv/channels/{id}/favorite",
    params(("id" = i64, Path, description = "Channel id")),
    responses((status = 204, description = "Removed"), (status = 404, body = ErrorBody))
)]
pub async fn remove_favorite(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    store::set_favorite(&st.db, u.id(), id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────────────────────── tune / live ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LiveModeDto {
    /// Raw MPEG-TS (`stream.ts`) for native players: zero server CPU.
    Passthrough,
    /// fMP4 HLS (`index.m3u8`) for browsers: video copied, audio re-encoded to AAC only if needed.
    Hls,
    /// The client plays the provider's own stream. `url` in the tune response is the provider URL
    /// (it contains the provider login) and the client must call `POST /iptv/sessions/{id}/keepalive`
    /// about every 15 s. Needs `REEL_ALLOW_DIRECT_LIVE`; the connection still counts against the
    /// provider's budget.
    Direct,
}

impl From<LiveModeDto> for LiveMode {
    fn from(m: LiveModeDto) -> Self {
        match m {
            LiveModeDto::Passthrough => LiveMode::Passthrough,
            LiveModeDto::Hls => LiveMode::Hls,
            LiveModeDto::Direct => LiveMode::Direct,
        }
    }
}

impl From<LiveMode> for LiveModeDto {
    fn from(m: LiveMode) -> Self {
        match m {
            LiveMode::Passthrough => LiveModeDto::Passthrough,
            LiveMode::Hls => LiveModeDto::Hls,
            LiveMode::Direct => LiveModeDto::Direct,
        }
    }
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct TuneBody {
    /// `hls` (default), `passthrough` or `direct` (403 unless the server allows it).
    pub mode: Option<LiveModeDto>,
    /// Built-in device preset (decides which codecs HLS may copy).
    pub preset: Option<String>,
    pub profile: Option<DeviceProfile>,
    /// Admins only: if every provider connection is held by other users, stop their streams and
    /// take the slot (answer to a 409 with `takeover: true`).
    pub takeover: Option<bool>,
    /// Chromecast/AirPlay receiver: defaults the preset to `chromecast` and returns an absolute,
    /// 8-hour signed URL on `cast_base_url`.
    pub for_cast: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HlsDetailDto {
    /// `copy` | `transcode` | `none`
    pub video: String,
    /// `copy` | `aac` | `none`
    pub audio: String,
    /// `copy`, or the video encoder backend (`vaapi`, `qsv`, `nvenc`, `videotoolbox`, `software`).
    pub encoder: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TuneResponse {
    pub session_id: String,
    pub mode: LiveModeDto,
    /// Signed URL: `stream.ts` (passthrough) or `index.m3u8` (HLS). Valid for the configured
    /// signed-URL lifetime; the session itself ends shortly after the player disconnects.
    /// For `direct`: the provider's own URL (contains the provider login; never log it).
    pub url: String,
    pub channel_id: i64,
    pub channel_name: String,
    pub hls: Option<HlsDetailDto>,
}

fn snake<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Start watching a channel: reserves a provider slot (429 if every slot is watched), returns a
/// signed player URL. Zapping is near-instant when the channel is already open or warm.
#[utoipa::path(
    post,
    path = "/iptv/channels/{id}/tune",
    params(("id" = i64, Path, description = "Channel id")),
    request_body = TuneBody,
    responses(
        (status = 200, body = TuneResponse),
        (status = 404, body = ErrorBody),
        (status = 429, description = "Provider connection limit, user stream limit, or user/server transcode limit reached", body = ErrorBody),
        (status = 503, description = "Channel did not deliver a stream", body = ErrorBody)
    )
)]
pub async fn tune(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
    body: Option<Json<TuneBody>>,
) -> ApiResult<Json<TuneResponse>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    if let Some(max) = u.user.limits.max_streams {
        let playing = st
            .playback
            .list()
            .await
            .into_iter()
            .filter(|s| s.user_id == u.id())
            .count();
        // newest wins: a user's older live sessions never block their new channel
        while (playing + st.iptv.live.count_for_user(u.id())) as i64 >= max
            && st.iptv.live.stop_oldest_of_user(u.id()).await
        {}
        let active = playing + st.iptv.live.count_for_user(u.id());
        if active as i64 >= max {
            return Err(ApiError::TooManyRequests(format!(
                "You reached your limit of {max} concurrent stream(s); stop another stream first"
            )));
        }
    }
    let cast = crate::prefs::cast_ctx(&st, b.for_cast.unwrap_or(false))?;
    let profile = match (b.profile, b.preset.as_deref()) {
        (Some(p), _) => Some(crate::prefs::sanitize_profile(&u.user, p)),
        (None, Some(name)) => Some(DeviceProfile::preset(name).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "unknown preset '{name}' (available: {})",
                DeviceProfile::preset_names().join(", ")
            ))
        })?),
        (None, None) if cast.is_some() => DeviceProfile::preset("chromecast"),
        (None, None) => None,
    };
    let opts = profile.map_or_else(HlsOptions::default, |p| HlsOptions {
        video_copy: p.video.iter().map(|v| v.codec.clone()).collect(),
        audio_copy: p.audio.iter().map(|a| a.codec.clone()).collect(),
    });
    let mode: LiveMode = b.mode.unwrap_or(LiveModeDto::Hls).into();
    if mode == LiveMode::Direct {
        if !st.config.allow_direct_live {
            return Err(ApiError::Forbidden(
                "Direct Live TV is not enabled on this server (REEL_ALLOW_DIRECT_LIVE)".into(),
            ));
        }
        if cast.is_some() {
            return Err(ApiError::BadRequest(
                "direct playback can't be combined with for_cast".into(),
            ));
        }
    }
    let takeover = u.is_admin() && b.takeover.unwrap_or(false);
    // a live HLS session that re-encodes video is a transcode like a playback one (#13)
    let (user_max, global_max) = st.playback.transcode_caps(
        u.user
            .limits
            .max_transcodes
            .and_then(|v| usize::try_from(v).ok()),
    );
    let limits = reel_iptv::TranscodeLimits {
        user_max,
        global_max,
    };
    let s = match st
        .iptv
        .live
        .tune_limited(u.id(), id, mode, &opts, takeover, limits)
        .await
    {
        Ok(s) => s,
        Err(reel_iptv::Error::InUseByOthers {
            source_id,
            max,
            holders,
        }) => return Err(in_use_error(&st, &u, source_id, max, &holders).await),
        Err(e) => return Err(e.into()),
    };
    // the tuned channel's guide (lazy per-channel providers)
    {
        let epg = st.iptv.epg.clone();
        tokio::spawn(async move { epg.ensure(&[id], std::time::Duration::ZERO).await });
    }
    if takeover {
        st.audit(
            &u.user,
            "iptv_takeover",
            Some(id.to_string()),
            json!({"channel_id": id}),
        )
        .await;
    }
    let url = match (mode, s.direct_url.clone()) {
        (LiveMode::Direct, Some(provider_url)) => {
            st.audit(
                &u.user,
                "iptv_direct",
                Some(id.to_string()),
                json!({"channel_id": id}),
            )
            .await;
            provider_url
        }
        _ => {
            let file = match mode {
                LiveMode::Hls => "index.m3u8",
                _ => "stream.ts",
            };
            let url = st.signer(u.id()).url(
                &format!("/api/v1/iptv/live/{}/{file}", s.id),
                &format!("live:{}", s.id),
                if cast.is_some() {
                    crate::prefs::Cast::ttl(&st)
                } else {
                    st.media_ttl()
                },
                1,
            );
            match &cast {
                Some(c) => c.abs(&url),
                None => url,
            }
        }
    };
    Ok(Json(TuneResponse {
        session_id: s.id,
        mode: s.mode.into(),
        url,
        channel_id: s.channel_id,
        channel_name: s.channel_name,
        hls: s.hls.map(|h| HlsDetailDto {
            video: snake(&h.video),
            audio: snake(&h.audio),
            encoder: h.encoder.to_string(),
        }),
    }))
}

/// Admins get a 409 with the holders and `takeover: true`; everyone else a friendly 429.
pub(crate) async fn in_use_error(
    st: &AppState,
    u: &AuthUser,
    _source_id: i64,
    max: u32,
    holders: &[reel_iptv::Holder],
) -> ApiError {
    if !u.is_admin() {
        return ApiError::TooManyRequests(crate::error::in_use_message(max));
    }
    let mut out = vec![];
    for h in holders {
        let name: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
            .bind(h.user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
        out.push(json!({
            "user_id": h.user_id,
            "username": name,
            "session_id": h.session_id,
            "channel_id": h.channel_id,
            "channel_name": h.channel_name,
            "since": h.since,
        }));
    }
    ApiError::Json(
        StatusCode::CONFLICT,
        json!({
            "error": format!(
                "All {max} connection(s) of this provider are in use by other users. \
                 Retry with takeover=true to stop their stream."
            ),
            "takeover": true,
            "holders": out,
        }),
    )
}

/// Stop watching. Passthrough viewers may also just disconnect; HLS sessions expire when the
/// player stops requesting segments.
#[utoipa::path(
    delete,
    path = "/iptv/sessions/{id}",
    params(("id" = String, Path, description = "Live session id")),
    responses((status = 204, description = "Stopped"), (status = 404, body = ErrorBody))
)]
pub async fn stop_session(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let s = st.iptv.live.session(&id).ok_or(ApiError::NotFound)?;
    if s.user_id != u.id() && !u.is_admin() {
        return Err(ApiError::NotFound);
    }
    // the provider slot goes back at once (no grace) when the provider has a connection limit
    st.iptv.live.stop_and_release(&id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Heartbeat of a `direct` session (the server doesn't see its video). Without one for 45 s the
/// session ends and its provider slot is released. 404 means the session is gone: tune again.
#[utoipa::path(
    post,
    path = "/iptv/sessions/{id}/keepalive",
    params(("id" = String, Path, description = "Live session id")),
    responses((status = 204, description = "Alive"), (status = 404, body = ErrorBody))
)]
pub async fn keepalive_session(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let s = st.iptv.live.session(&id).ok_or(ApiError::NotFound)?;
    if s.user_id != u.id() {
        return Err(ApiError::NotFound);
    }
    if st.iptv.live.keepalive(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Live output of a session: `stream.ts` (MPEG-TS passthrough) or the HLS playlist / init /
/// segments. Bearer token (session owner or admin) or a signature for scope `live:{session}`.
#[utoipa::path(
    get,
    path = "/iptv/live/{session_id}/{file}",
    params(
        ("session_id" = String, Path, description = "Session id from tune"),
        ("file" = String, Path, description = "`stream.ts`, `index.m3u8`, `init.mp4` or `seg_NNNNN.m4s`"),
        SignedQuery
    ),
    responses(
        (status = 200, description = "Media bytes or playlist"),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 503, description = "HLS output not available", body = ErrorBody)
    )
)]
pub async fn live_file(
    State(st): State<AppState>,
    Path((sid, file)): Path<(String, String)>,
    Query(sq): Query<SignedQuery>,
    crate::auth::Peer(peer): crate::auth::Peer,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let user = st
        .media_user_at(peer, &headers, &sq, &format!("live:{sid}"))
        .await?;
    let info = st.iptv.live.session(&sid).ok_or(ApiError::NotFound)?;
    if info.user_id != user.id && !user.is_admin() {
        return Err(ApiError::NotFound);
    }
    if file == "stream.ts" {
        let stream = st.iptv.live.ts_stream(&sid).await?;
        let mut res = Body::from_stream(stream).into_response();
        let h = res.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(res);
    }
    let path = st.iptv.live.hls_file(&sid, &file).await?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let playlist = file.ends_with(".m3u8");
    let body: Body = match (playlist, sq.exp, sq.u, sq.sig.as_deref()) {
        // signed playlists: propagate the signature to every relative URI
        (true, Some(exp), Some(u), Some(sig)) if crate::auth::bearer(&headers).is_none() => {
            Body::from(crate::stream::sign_playlist(
                &String::from_utf8_lossy(&bytes),
                &format!("exp={exp}&u={u}&sig={sig}"),
            ))
        }
        _ => Body::from(bytes),
    };
    let mut res = body.into_response();
    let h = res.headers_mut();
    let (ct, cc) = if playlist {
        ("application/vnd.apple.mpegurl", "no-cache")
    } else {
        ("video/mp4", "max-age=30")
    };
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    Ok(res)
}

// ───────────────────────── logos ─────────────────────────

/// Download (once) and cache a remote image: SSRF rules of channel logos apply (private targets
/// refused unless `allow_private_logo_fetch`); size, time and disk budgets and single-flight are
/// in [`crate::remote_images`]. Only real images are served.
pub(crate) async fn fetch_image_cached(
    st: &AppState,
    url: &str,
    cache_dir: &str,
) -> ApiResult<(Vec<u8>, &'static str)> {
    if reel_iptv::net::check_url(url, st.config.allow_private_logo_fetch).is_err() {
        return Err(ApiError::NotFound);
    }
    st.remote_images
        .get(&st.iptv.logo_http, url, cache_dir)
        .await
        .ok_or(ApiError::NotFound)
}

/// Channel logo, fetched once from the provider and cached on disk.
#[utoipa::path(
    get,
    path = "/iptv/logo/{id}",
    params(("id" = i64, Path, description = "Channel id"), SignedQuery),
    responses((status = 200, description = "Image", content_type = "image/png"), (status = 404, body = ErrorBody))
)]
pub async fn channel_logo(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(sq): Query<SignedQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    st.media_user(&headers, &sq, &format!("logo:{id}")).await?;
    let url: String =
        sqlx::query_scalar("SELECT logo FROM channels WHERE id = ? AND logo IS NOT NULL")
            .bind(id)
            .fetch_optional(&st.db)
            .await?
            .ok_or(ApiError::NotFound)?;
    // Logos come from third-party playlists: private/loopback targets are refused by default.
    let (bytes, ct) = fetch_image_cached(&st, &url, "logos").await?;
    let mut res = Body::from(bytes).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    Ok(res)
}

// ───────────────────────── admin: sources ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceDto {
    pub id: i64,
    pub name: String,
    /// `m3u` | `xtream`
    pub kind: String,
    /// Playlist / server URL with credentials masked (`***`: userinfo, `username`/`password`-style
    /// query parameters, Xtream `/live/USER/PASS/` path segments). Sending it back unchanged in a
    /// patch keeps the saved URL.
    pub url: String,
    /// Xtream username, masked (`ab••••`, or `••••` when short). Never returned in clear: to change
    /// it, send the new one; sending the masked value back keeps the saved one.
    pub username: Option<String>,
    pub password_set: bool,
    /// Provider connection limit (`null` = unlimited / unknown).
    pub max_connections: Option<u32>,
    /// Deprecated: first entry of `epg_urls` (credentials masked).
    pub epg_url: Option<String>,
    /// External guide (XMLTV) URLs in precedence order, credentials masked like `url`. The
    /// provider's own guide (when it has one) is imported first, then each of these; for a channel
    /// carried by several guides the earlier one wins. A patch may send masked entries back
    /// unchanged; they keep their saved URL.
    pub epg_urls: Vec<String>,
    /// Import result per guide (provider guide first, then `epg_urls` in order).
    pub epg_sources: Vec<EpgSourceDto>,
    pub detected_epg_url: Option<String>,
    /// Minutes between refreshes; `null` = default (12 h); 0 = manual only.
    pub refresh_interval: Option<i64>,
    pub user_agent: Option<String>,
    pub enabled: bool,
    pub channel_count: i64,
    pub last_refresh_at: Option<i64>,
    pub last_epg_at: Option<i64>,
    pub last_error: Option<String>,
    /// Channels in the guide delivered by the last import.
    pub epg_channel_count: i64,
    /// Programmes stored for this source's channels.
    pub epg_programme_count: i64,
    /// This source's channels linked to a guide channel (tvg-id or name match).
    pub epg_matched_channels: i64,
    /// Where programmes come from: `xmltv` (feed imported), `per_channel` (Xtream `get_short_epg`,
    /// fetched lazily for the channels people look at) or `none`.
    pub epg_mode: String,
    /// Why the XMLTV feed was not usable (`per_channel` mode).
    pub epg_note: Option<String>,
    /// Xtream VOD catalog size.
    pub vod_movie_count: i64,
    pub vod_series_count: i64,
    pub refreshing: bool,
    /// Upstream connections open right now (live + warm).
    pub open_connections: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EpgSourceDto {
    /// Guide URL (credentials masked).
    pub url: String,
    /// The provider's own XMLTV (Xtream `xmltv.php` / playlist `x-tvg-url`) rather than an added URL.
    pub provider: bool,
    /// Guide channels this import delivered (channels already carried by an earlier guide excluded).
    pub channels: i64,
    /// Programmes stored from this guide.
    pub programmes: i64,
    /// This source's channels that end up using a channel of this guide (the earliest guide that
    /// has programmes for a channel supplies it).
    pub matched: i64,
    /// This source's channels that have a name/id match in this guide, even if an earlier guide
    /// supplies them.
    pub available: i64,
    pub last_error: Option<String>,
    pub last_at: Option<i64>,
}

async fn epg_sources(st: &AppState, s: &Source) -> Vec<EpgSourceDto> {
    let rows = store::guide_stats(&st.db, s.id).await.unwrap_or_default();
    let mut out: Vec<EpgSourceDto> = vec![];
    let dto = |r: &store::GuideStat| EpgSourceDto {
        url: reel_iptv::net::redact_url(&r.url),
        provider: r.provider,
        channels: r.channels,
        programmes: r.programmes,
        matched: r.matched,
        available: r.available,
        last_error: r.last_error.as_deref().map(reel_iptv::net::redact_text),
        last_at: r.last_at,
    };
    if let Some(p) = rows.iter().find(|r| r.provider) {
        out.push(dto(p));
    }
    // external guides: positional (rank = index + 1), listed even before their first import
    let ext: Vec<&store::GuideStat> = rows.iter().filter(|r| !r.provider).collect();
    for (i, u) in s.epg_urls.iter().enumerate() {
        let shown = reel_iptv::net::redact_url(u);
        match ext.iter().find(|r| r.url == shown) {
            Some(r) => out.push(dto(r)),
            None => {
                let _ = i;
                out.push(EpgSourceDto {
                    url: shown,
                    provider: false,
                    channels: 0,
                    programmes: 0,
                    matched: 0,
                    available: 0,
                    last_error: None,
                    last_at: None,
                })
            }
        }
    }
    out
}

/// Masked form of a provider username: the first two characters of a long one, else nothing.
pub(crate) fn mask_username(u: &str) -> String {
    let n = u.chars().count();
    if n < 6 {
        "••••".into()
    } else {
        format!("{}••••", u.chars().take(2).collect::<String>())
    }
}

/// A patched URL: the masked form of `current` sent back unchanged keeps `current`; anything still
/// holding a mask (`***`) is refused, because the real value can't be recovered from it.
fn unmask_url(sent: &str, current: &[String]) -> ApiResult<String> {
    let sent = sent.trim();
    if let Some(c) = current
        .iter()
        .find(|c| c.as_str() == sent || reel_iptv::net::redact_url(c) == sent)
    {
        return Ok(c.clone());
    }
    if sent.contains("***") {
        return Err(ApiError::BadRequest(
            "a URL still contains a hidden part (***): enter the full URL, credentials included"
                .into(),
        ));
    }
    Ok(sent.to_string())
}

/// `scheme://host:port` of a URL, to notice a source moving to another server.
fn origin(u: &str) -> Option<String> {
    reqwest::Url::parse(u)
        .ok()
        .map(|u| u.origin().ascii_serialization())
}

async fn source_dto(st: &AppState, s: &Source) -> SourceDto {
    let redact = |u: &String| reel_iptv::net::redact_url(u);
    SourceDto {
        epg_urls: s.epg_urls.iter().map(redact).collect(),
        epg_sources: epg_sources(st, s).await,
        id: s.id,
        name: s.name.clone(),
        kind: s.kind.as_str().into(),
        url: redact(&s.url),
        username: s.username.as_deref().map(mask_username),
        password_set: s.password.as_deref().is_some_and(|p| !p.is_empty()),
        max_connections: s.max_connections,
        epg_url: s.epg_urls.first().map(redact),
        // credentials in the guide URL (Xtream `xmltv.php?username=…&password=…`) are masked
        detected_epg_url: s
            .detected_epg_url
            .as_deref()
            .map(reel_iptv::net::redact_url),
        refresh_interval: s.refresh_interval,
        user_agent: s.user_agent.clone(),
        enabled: s.enabled,
        channel_count: s.channel_count,
        last_refresh_at: s.last_refresh_at,
        last_epg_at: s.last_epg_at,
        last_error: s.last_error.as_deref().map(reel_iptv::net::redact_text),
        epg_channel_count: s.epg_channel_count,
        epg_programme_count: s.epg_programme_count,
        epg_matched_channels: s.epg_matched_channels,
        epg_mode: s.epg_mode.clone(),
        epg_note: s.epg_note.as_deref().map(reel_iptv::net::redact_text),
        vod_movie_count: s.vod_movie_count,
        vod_series_count: s.vod_series_count,
        refreshing: st.iptv.is_refreshing(s.id),
        open_connections: st
            .iptv
            .hub()
            .connections_by_source()
            .get(&s.id)
            .copied()
            .unwrap_or(0),
    }
}

/// All IPTV sources.
#[utoipa::path(get, path = "/admin/iptv/sources", responses((status = 200, body = Vec<SourceDto>)))]
pub async fn list_sources(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<SourceDto>>> {
    let s = store::list_sources(&st.db).await?;
    let mut out = Vec::with_capacity(s.len());
    for x in &s {
        out.push(source_dto(&st, x).await);
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SourceBody {
    pub name: String,
    /// `m3u` | `xtream`
    pub kind: String,
    /// Playlist URL (m3u) or Xtream server base URL.
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub max_connections: Option<u32>,
    /// Deprecated: same as `epg_urls: [epg_url]`.
    pub epg_url: Option<String>,
    /// External guide (XMLTV, optionally .gz) URLs in precedence order.
    pub epg_urls: Option<Vec<String>>,
    pub refresh_interval: Option<i64>,
    pub user_agent: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn validate_url(st: &AppState, u: &str) -> ApiResult<()> {
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err(ApiError::BadRequest(
            "url must start with http:// or https://".into(),
        ));
    }
    // `***` is the mask admin responses use: such a URL would send the mask to the provider
    if u.contains("***") {
        return Err(ApiError::BadRequest(
            "a URL still contains a hidden part (***): enter the full URL, credentials included"
                .into(),
        ));
    }
    reel_iptv::net::check_url(u, st.config.allow_private_fetch)?;
    Ok(())
}

/// New guide list from `epg_urls` (preferred) or the deprecated `epg_url` (empty string clears);
/// neither given keeps `current`. Trimmed, de-duplicated, each validated like any fetched URL.
fn resolve_urls(
    st: &AppState,
    urls: Option<Vec<String>>,
    legacy: Option<String>,
    current: &[String],
) -> ApiResult<Vec<String>> {
    let Some(list) = urls.or_else(|| legacy.map(|u| vec![u])) else {
        return Ok(current.to_vec());
    };
    let mut out: Vec<String> = vec![];
    for u in list.iter().map(|u| u.trim()).filter(|u| !u.is_empty()) {
        validate_url(st, u)?;
        if !out.iter().any(|x| x == u) {
            out.push(u.to_string());
        }
    }
    if out.len() > 20 {
        return Err(ApiError::BadRequest("at most 20 guide URLs".into()));
    }
    Ok(out)
}

fn urls_json(urls: &[String]) -> Option<String> {
    (!urls.is_empty()).then(|| serde_json::to_string(urls).unwrap_or_default())
}

fn validate_source(st: &AppState, b: &SourceBody) -> ApiResult<SourceKind> {
    let kind = SourceKind::parse(&b.kind)
        .ok_or_else(|| ApiError::BadRequest("kind must be m3u or xtream".into()))?;
    if b.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    validate_url(st, b.url.trim())?;
    resolve_urls(st, b.epg_urls.clone(), b.epg_url.clone(), &[])?;
    if kind == SourceKind::Xtream
        && (b.username.as_deref().is_none_or(str::is_empty)
            || b.password.as_deref().is_none_or(str::is_empty))
    {
        return Err(ApiError::BadRequest(
            "Xtream sources need username and password".into(),
        ));
    }
    Ok(kind)
}

/// Add a source and start the first refresh in the background.
#[utoipa::path(
    post,
    path = "/admin/iptv/sources",
    request_body = SourceBody,
    responses((status = 201, body = SourceDto), (status = 400, body = ErrorBody))
)]
pub async fn create_source(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(b): Json<SourceBody>,
) -> ApiResult<(StatusCode, Json<SourceDto>)> {
    let kind = validate_source(&st, &b)?;
    let urls = resolve_urls(&st, b.epg_urls.clone(), b.epg_url.clone(), &[])?;
    let id = sqlx::query(
        "INSERT INTO iptv_sources (name, kind, url, username, password, max_connections, epg_url, \
         epg_urls, refresh_interval, user_agent, enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(b.name.trim())
    .bind(kind.as_str())
    .bind(b.url.trim())
    .bind(&b.username)
    .bind(&b.password)
    .bind(b.max_connections.map(i64::from))
    .bind(urls.first())
    .bind(urls_json(&urls))
    .bind(b.refresh_interval)
    .bind(b.user_agent.as_deref().filter(|e| !e.is_empty()))
    .bind(i64::from(b.enabled))
    .execute(&st.db)
    .await?
    .last_insert_rowid();
    st.audit(
        &a.user,
        "iptv_source_create",
        Some(id.to_string()),
        json!({"name": b.name, "kind": b.kind}),
    )
    .await;
    if b.enabled {
        st.iptv.spawn_refresh(id);
    }
    let s = store::get_source(&st.db, id).await?;
    Ok((StatusCode::CREATED, Json(source_dto(&st, &s).await)))
}

/// One source.
#[utoipa::path(
    get,
    path = "/admin/iptv/sources/{id}",
    params(("id" = i64, Path, description = "Source id")),
    responses((status = 200, body = SourceDto), (status = 404, body = ErrorBody))
)]
pub async fn get_source(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<SourceDto>> {
    Ok(Json(
        source_dto(&st, &store::get_source(&st.db, id).await?).await,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SourcePatch {
    pub name: Option<String>,
    /// New URL. The masked `url` of the source sent back unchanged keeps the saved one; a URL that
    /// still contains `***` is refused. Moving an Xtream source to another server
    /// (scheme/host/port) needs `username` and `password` in the same patch, so the saved login is
    /// never sent to a server the admin didn't type it for.
    pub url: Option<String>,
    /// Omit (or send the masked value back) to keep the current username; `""` clears it.
    pub username: Option<String>,
    /// Omit to keep the current password.
    pub password: Option<String>,
    /// Use `0` for "unlimited".
    pub max_connections: Option<u32>,
    /// Deprecated: replaces the list with this single URL (empty string clears).
    pub epg_url: Option<String>,
    /// Replace the external guide URL list (empty list clears). Omit to keep it. Entries may be the
    /// masked URLs from `epg_urls`; they keep their saved URL.
    pub epg_urls: Option<Vec<String>>,
    pub refresh_interval: Option<i64>,
    pub user_agent: Option<String>,
    pub enabled: Option<bool>,
}

/// Update a source (only the provided fields).
#[utoipa::path(
    patch,
    path = "/admin/iptv/sources/{id}",
    params(("id" = i64, Path, description = "Source id")),
    request_body = SourcePatch,
    responses((status = 200, body = SourceDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn update_source(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    Json(p): Json<SourcePatch>,
) -> ApiResult<Json<SourceDto>> {
    let cur = store::get_source(&st.db, id).await?;
    let mut p = p;
    // Masked values sent back unchanged mean "keep": the DTO never holds the real ones.
    let url = match p.url.as_deref() {
        Some(u) => unmask_url(u, std::slice::from_ref(&cur.url))?,
        None => cur.url.clone(),
    };
    if url != cur.url {
        validate_url(&st, &url)?;
    }
    if p.username.as_deref().is_some_and(|u| {
        cur.username
            .as_deref()
            .is_some_and(|c| u == mask_username(c))
    }) {
        p.username = None;
    }
    if let Some(list) = p.epg_urls.as_mut() {
        for u in list.iter_mut() {
            *u = unmask_url(u, &cur.epg_urls)?;
        }
    }
    if let Some(u) = p.epg_url.as_mut() {
        *u = unmask_url(u, &cur.epg_urls)?;
    }
    let new_login = p.username.as_deref().is_some_and(|u| !u.is_empty())
        && p.password.as_deref().is_some_and(|p| !p.is_empty());
    if cur.kind == SourceKind::Xtream && origin(&url) != origin(&cur.url) && !new_login {
        return Err(ApiError::BadRequest(
            "moving the source to another server needs the username and password again".into(),
        ));
    }
    let urls = resolve_urls(&st, p.epg_urls.clone(), p.epg_url.clone(), &cur.epg_urls)?;
    let empty_to_none = |s: Option<String>, cur: Option<String>| match s {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(v),
        None => cur,
    };
    sqlx::query(
        "UPDATE iptv_sources SET name = ?, url = ?, username = ?, password = ?, max_connections = ?, \
         epg_url = ?, epg_urls = ?, refresh_interval = ?, user_agent = ?, enabled = ? WHERE id = ?",
    )
    .bind(p.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&cur.name))
    .bind(&url)
    .bind(empty_to_none(p.username, cur.username))
    .bind(empty_to_none(p.password, cur.password))
    .bind(match p.max_connections {
        Some(0) => None,
        Some(n) => Some(i64::from(n)),
        None => cur.max_connections.map(i64::from),
    })
    .bind(urls.first())
    .bind(urls_json(&urls))
    .bind(p.refresh_interval.or(cur.refresh_interval))
    .bind(empty_to_none(p.user_agent, cur.user_agent))
    .bind(i64::from(p.enabled.unwrap_or(cur.enabled)))
    .bind(id)
    .execute(&st.db)
    .await?;
    st.audit(
        &a.user,
        "iptv_source_update",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(Json(
        source_dto(&st, &store::get_source(&st.db, id).await?).await,
    ))
}

/// Delete a source and all its channels.
#[utoipa::path(
    delete,
    path = "/admin/iptv/sources/{id}",
    params(("id" = i64, Path, description = "Source id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorBody))
)]
pub async fn delete_source(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let n = sqlx::query("DELETE FROM iptv_sources WHERE id = ?")
        .bind(id)
        .execute(&st.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    st.audit(
        &a.user,
        "iptv_source_delete",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Refresh channels and EPG now (runs in the background; poll the source for `refreshing`,
/// `last_refresh_at` and `last_error`).
#[utoipa::path(
    post,
    path = "/admin/iptv/sources/{id}/refresh",
    params(("id" = i64, Path, description = "Source id")),
    responses((status = 202, description = "Started"), (status = 404, body = ErrorBody), (status = 409, description = "Already running", body = ErrorBody))
)]
pub async fn refresh_source(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    store::get_source(&st.db, id).await?;
    if st.iptv.is_refreshing(id) {
        return Err(ApiError::Status(
            StatusCode::CONFLICT,
            "a refresh is already running".into(),
        ));
    }
    st.audit(
        &a.user,
        "iptv_source_refresh",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    st.iptv.spawn_refresh(id);
    Ok(StatusCode::ACCEPTED)
}

/// Re-run only the guide (EPG) import of a source, in the background. Poll the source:
/// `refreshing`, `last_epg_at`, `last_error` (set when the guide was empty or unusable) and the
/// `epg_*` counters.
#[utoipa::path(
    post,
    path = "/admin/iptv/sources/{id}/refresh-epg",
    params(("id" = i64, Path, description = "Source id")),
    responses((status = 202, description = "Started"), (status = 404, body = ErrorBody), (status = 409, description = "A refresh is already running", body = ErrorBody))
)]
pub async fn refresh_source_epg(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    store::get_source(&st.db, id).await?;
    if st.iptv.is_refreshing(id) {
        return Err(ApiError::Status(
            StatusCode::CONFLICT,
            "a refresh is already running".into(),
        ));
    }
    st.audit(
        &a.user,
        "iptv_source_refresh_epg",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    st.iptv.spawn_epg_refresh(id);
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountDto {
    pub active: bool,
    pub status: Option<String>,
    pub exp_date: Option<i64>,
    pub max_connections: Option<u32>,
    pub active_connections: Option<u32>,
    pub is_trial: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceTest {
    pub ok: bool,
    /// Human-readable outcome (or the failure reason).
    pub message: String,
    /// Channels found (M3U: all; Xtream: live streams).
    pub channels: Option<usize>,
    /// Xtream account details.
    pub account: Option<AccountDto>,
    /// EPG URL advertised by the playlist.
    pub epg_url: Option<String>,
}

async fn run_test(
    st: &AppState,
    kind: SourceKind,
    url: &str,
    user: Option<&str>,
    pass: Option<&str>,
    ua: Option<&str>,
) -> SourceTest {
    let fail = |m: String| SourceTest {
        ok: false,
        message: m,
        channels: None,
        account: None,
        epg_url: None,
    };
    match kind {
        SourceKind::M3u => {
            let mut req = st
                .iptv
                .ingest
                .http
                .get(url)
                .timeout(std::time::Duration::from_secs(60));
            req = req.header("User-Agent", ua.unwrap_or(reel_iptv::net::DEFAULT_UA));
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => return fail(format!("cannot reach the server: {}", e.without_url())),
            };
            if !resp.status().is_success() {
                return fail(format!(
                    "the server answered with HTTP {}",
                    resp.status().as_u16()
                ));
            }
            let body = match reel_iptv::limits::read_body(
                resp,
                "the playlist",
                reel_iptv::limits::PLAYLIST_MAX_BYTES,
            )
            .await
            {
                Ok(b) => b,
                Err(e) => return fail(format!("download failed: {e}")),
            };
            let (entries, epg) = match reel_iptv::m3u::parse(&String::from_utf8_lossy(&body)) {
                Ok(r) => r,
                Err(e) => return fail(e.to_string()),
            };
            if entries.is_empty() {
                return fail("the playlist contains no channels (is this an M3U URL?)".into());
            }
            SourceTest {
                ok: true,
                message: format!("{} channels found", entries.len()),
                channels: Some(entries.len()),
                account: None,
                epg_url: epg.as_deref().map(reel_iptv::net::redact_url),
            }
        }
        SourceKind::Xtream => {
            let (Some(u), Some(p)) = (user, pass) else {
                return fail("username and password are required".into());
            };
            let x = XtreamClient::new(st.iptv.ingest.http.clone(), url, u, p)
                .with_user_agent(ua.map(str::to_string));
            let acc = match x.account_info().await {
                Ok(a) => a,
                Err(e) => return fail(e.to_string()),
            };
            let n = x.live_streams().await.map(|s| s.len()).ok();
            SourceTest {
                ok: acc.active,
                message: if acc.active {
                    format!(
                        "account active, {} connection(s) allowed",
                        acc.max_connections
                            .map_or("?".to_string(), |m| m.to_string())
                    )
                } else {
                    format!(
                        "account is not active ({})",
                        acc.status.clone().unwrap_or_default()
                    )
                },
                channels: n,
                epg_url: Some(
                    x.xmltv_url()
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                ),
                account: Some(AccountDto {
                    active: acc.active,
                    status: acc.status,
                    exp_date: acc.exp_date,
                    max_connections: acc.max_connections,
                    active_connections: acc.active_connections,
                    is_trial: acc.is_trial,
                }),
            }
        }
    }
}

/// Try a source configuration without saving it.
#[utoipa::path(
    post,
    path = "/admin/iptv/sources/test",
    request_body = SourceBody,
    responses((status = 200, body = SourceTest), (status = 400, body = ErrorBody))
)]
pub async fn test_new_source(
    State(st): State<AppState>,
    _a: AdminUser,
    Json(b): Json<SourceBody>,
) -> ApiResult<Json<SourceTest>> {
    let kind = validate_source(&st, &b)?;
    Ok(Json(
        run_test(
            &st,
            kind,
            b.url.trim(),
            b.username.as_deref(),
            b.password.as_deref(),
            b.user_agent.as_deref(),
        )
        .await,
    ))
}

/// Re-test a saved source: reachability, credentials, channel count, Xtream account info
/// (expiry, allowed and active connections).
#[utoipa::path(
    post,
    path = "/admin/iptv/sources/{id}/test",
    params(("id" = i64, Path, description = "Source id")),
    responses((status = 200, body = SourceTest), (status = 404, body = ErrorBody))
)]
pub async fn test_source(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<SourceTest>> {
    let s = store::get_source(&st.db, id).await?;
    Ok(Json(
        run_test(
            &st,
            s.kind,
            &s.url,
            s.username.as_deref(),
            s.password.as_deref(),
            s.user_agent.as_deref(),
        )
        .await,
    ))
}

// ───────────────────────── admin: channels ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminChannel {
    pub id: i64,
    pub source_id: i64,
    pub name: String,
    pub number: Option<i64>,
    pub group: Option<String>,
    pub tvg_id: Option<String>,
    /// XMLTV channel id the guide is read from.
    pub epg_channel_id: Option<String>,
    /// The mapping was set manually and is not recomputed.
    pub epg_manual: bool,
    pub enabled: bool,
    /// No longer in the provider's list (hidden).
    pub missing: bool,
    pub catchup: bool,
    /// Failover aliases `(channel id, priority)`.
    pub aliases: Vec<AliasDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AliasDto {
    pub channel_id: i64,
    /// Lower is tried first.
    pub priority: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AdminChannelsQuery {
    pub source: Option<i64>,
    pub q: Option<String>,
    pub group: Option<String>,
    /// Include disabled and vanished channels (default true in the admin view).
    pub hidden: Option<bool>,
}

/// Channels as the admin sees them (including disabled / missing).
#[utoipa::path(get, path = "/admin/iptv/channels", params(AdminChannelsQuery), responses((status = 200, body = Vec<AdminChannel>)))]
pub async fn admin_channels(
    State(st): State<AppState>,
    _a: AdminUser,
    Query(q): Query<AdminChannelsQuery>,
) -> ApiResult<Json<Vec<AdminChannel>>> {
    let rows = store::list_channels(
        &st.db,
        &ChannelFilter {
            group: q.group,
            q: q.q.filter(|s| !s.trim().is_empty()),
            source_id: q.source,
            include_hidden: q.hidden.unwrap_or(true),
            ..Default::default()
        },
    )
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for c in rows {
        let aliases = store::get_aliases(&st.db, c.id)
            .await?
            .into_iter()
            .map(|(channel_id, priority)| AliasDto {
                channel_id,
                priority,
            })
            .collect();
        out.push(AdminChannel {
            id: c.id,
            source_id: c.source_id,
            name: c.name,
            number: c.number,
            group: c.group,
            tvg_id: c.tvg_id,
            epg_channel_id: c.epg_channel_id,
            epg_manual: c.epg_manual,
            enabled: c.enabled,
            missing: c.missing,
            catchup: c.has_catchup,
            aliases,
        });
    }
    Ok(Json(out))
}

/// Partial update; absent fields are unchanged, `null` clears where noted.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChannelPatchBody {
    pub enabled: Option<bool>,
    /// Rename; survives provider refreshes.
    pub name: Option<String>,
    /// Channel number; `null` removes the custom number.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub number: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub group: Option<Option<String>>,
    /// Manual XMLTV channel id; `null` returns to automatic matching.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub epg_channel_id: Option<Option<String>>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(d)?))
}

/// Enable/disable, rename, renumber, regroup or remap the EPG of a channel.
#[utoipa::path(
    patch,
    path = "/admin/iptv/channels/{id}",
    params(("id" = i64, Path, description = "Channel id")),
    request_body = ChannelPatchBody,
    responses((status = 204, description = "Updated"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn patch_channel(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<ChannelPatchBody>,
) -> ApiResult<StatusCode> {
    store::update_channel(
        &st.db,
        id,
        &ChannelPatch {
            enabled: b.enabled,
            name: b.name,
            number: b.number,
            group: b.group,
            epg_channel_id: b.epg_channel_id,
        },
    )
    .await?;
    st.audit(
        &a.user,
        "iptv_channel_update",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Replace the failover aliases of a channel (tried in priority order when it stalls).
#[utoipa::path(
    put,
    path = "/admin/iptv/channels/{id}/aliases",
    params(("id" = i64, Path, description = "Channel id")),
    request_body = Vec<AliasDto>,
    responses((status = 204, description = "Saved"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn set_aliases(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    Json(list): Json<Vec<AliasDto>>,
) -> ApiResult<StatusCode> {
    let pairs: Vec<(i64, i64)> = list.iter().map(|x| (x.channel_id, x.priority)).collect();
    store::set_aliases(&st.db, id, &pairs).await?;
    st.audit(
        &a.user,
        "iptv_aliases_set",
        Some(id.to_string()),
        json!({"count": pairs.len()}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ───────────────────────── admin: hub ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct ChannelMetricsDto {
    pub channel_id: i64,
    pub name: String,
    pub source_id: i64,
    /// `connecting` | `live` | `reconnecting`
    pub state: String,
    pub viewers: usize,
    /// Open without viewers (kept for instant zapping).
    pub warm: bool,
    pub pinned: bool,
    pub uptime_s: u64,
    pub connected_s: Option<u64>,
    pub bitrate_kbps: u64,
    pub bytes_total: u64,
    pub reconnects: u64,
    pub failovers: u64,
    /// 0 = primary, n = nth alias.
    pub active_candidate: usize,
    /// MPEG-TS continuity counter errors (packet loss indicator).
    pub cc_errors: u64,
    pub pts_discontinuities: u64,
    pub sync_losses: u64,
    pub keyframes: u64,
    /// Slow viewers skipped ahead to the latest keyframe.
    pub viewer_resyncs: u64,
    /// Pipe start → first upstream byte (ms).
    pub ttfb_ms: Option<u64>,
    /// Pipe start → first keyframe (ms).
    pub time_to_keyframe_ms: Option<u64>,
    pub gop_bytes: usize,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub last_error: Option<String>,
    pub upstream_host: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceUsage {
    pub source_id: i64,
    pub open_connections: usize,
    pub max_connections: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LiveSessionDto {
    pub id: String,
    pub user_id: i64,
    pub username: Option<String>,
    pub channel_id: i64,
    pub channel_name: String,
    pub mode: LiveModeDto,
    pub started_at: i64,
    pub idle_s: u64,
    /// Open `stream.ts` connections.
    pub connections: usize,
    pub hls: Option<HlsDetailDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TranscodeStatus {
    /// Configured encoder backend: `vaapi`, `qsv`, `nvenc`, `videotoolbox` or `software`.
    pub encoder: String,
    /// The hardware encoder failed several starts in a row (not merely once) and is disabled for
    /// a 10-minute cool-down; software encoding is used meanwhile.
    pub fell_back_to_software: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HubStatus {
    /// Video encoder used for IPTV transcodes (non-H.264 sources) and whether it fell back.
    pub transcode: TranscodeStatus,
    pub sources: Vec<SourceUsage>,
    pub channels: Vec<ChannelMetricsDto>,
    pub sessions: Vec<LiveSessionDto>,
}

async fn session_dtos(st: &AppState) -> ApiResult<Vec<LiveSessionDto>> {
    let mut out = Vec::new();
    for s in st.iptv.live.list() {
        let username: Option<String> =
            sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
                .bind(s.user_id)
                .fetch_optional(&st.db)
                .await?;
        out.push(LiveSessionDto {
            username,
            id: s.id,
            user_id: s.user_id,
            channel_id: s.channel_id,
            channel_name: s.channel_name,
            mode: s.mode.into(),
            started_at: s.started_at,
            idle_s: s.idle_s,
            connections: s.connections,
            hls: s.hls.map(|h| HlsDetailDto {
                video: snake(&h.video),
                audio: snake(&h.audio),
                encoder: h.encoder.to_string(),
            }),
        });
    }
    Ok(out)
}

/// Live pipeline health: open upstream connections per source (vs. the provider limit), per
/// channel bitrate / TTFB / reconnects / failovers / TS errors, and active viewer sessions.
#[utoipa::path(get, path = "/admin/iptv/hub", responses((status = 200, body = HubStatus)))]
pub async fn hub_status(State(st): State<AppState>, _a: AdminUser) -> ApiResult<Json<HubStatus>> {
    let open = st.iptv.hub().connections_by_source();
    let sources = store::list_sources(&st.db)
        .await?
        .into_iter()
        .map(|s| SourceUsage {
            source_id: s.id,
            open_connections: open.get(&s.id).copied().unwrap_or(0),
            max_connections: s.max_connections,
        })
        .collect();
    let channels = st
        .iptv
        .hub()
        .metrics()
        .into_iter()
        .map(|m| {
            let v = serde_json::to_value(&m).unwrap_or(Value::Null);
            serde_json::from_value::<ChannelMetricsDtoIn>(v)
                .map(Into::into)
                .expect("metrics shape")
        })
        .collect();
    let (encoder, failed) = st.iptv.live.hw_status();
    Ok(Json(HubStatus {
        transcode: TranscodeStatus {
            encoder: encoder.to_string(),
            fell_back_to_software: failed,
        },
        sources,
        channels,
        sessions: session_dtos(&st).await?,
    }))
}

/// Deserialization mirror of `reel_iptv::ChannelMetrics`.
#[derive(Deserialize)]
struct ChannelMetricsDtoIn {
    channel_id: i64,
    name: String,
    source_id: i64,
    state: String,
    viewers: usize,
    warm: bool,
    pinned: bool,
    uptime_s: u64,
    connected_s: Option<u64>,
    bitrate_kbps: u64,
    bytes_total: u64,
    reconnects: u64,
    failovers: u64,
    active_candidate: usize,
    cc_errors: u64,
    pts_discontinuities: u64,
    sync_losses: u64,
    keyframes: u64,
    viewer_resyncs: u64,
    ttfb_ms: Option<u64>,
    time_to_keyframe_ms: Option<u64>,
    gop_bytes: usize,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    last_error: Option<String>,
    upstream_host: Option<String>,
}

impl From<ChannelMetricsDtoIn> for ChannelMetricsDto {
    fn from(m: ChannelMetricsDtoIn) -> Self {
        ChannelMetricsDto {
            channel_id: m.channel_id,
            name: m.name,
            source_id: m.source_id,
            state: m.state,
            viewers: m.viewers,
            warm: m.warm,
            pinned: m.pinned,
            uptime_s: m.uptime_s,
            connected_s: m.connected_s,
            bitrate_kbps: m.bitrate_kbps,
            bytes_total: m.bytes_total,
            reconnects: m.reconnects,
            failovers: m.failovers,
            active_candidate: m.active_candidate,
            cc_errors: m.cc_errors,
            pts_discontinuities: m.pts_discontinuities,
            sync_losses: m.sync_losses,
            keyframes: m.keyframes,
            viewer_resyncs: m.viewer_resyncs,
            ttfb_ms: m.ttfb_ms,
            time_to_keyframe_ms: m.time_to_keyframe_ms,
            gop_bytes: m.gop_bytes,
            video_codec: m.video_codec,
            audio_codec: m.audio_codec,
            last_error: m.last_error,
            upstream_host: m.upstream_host,
        }
    }
}

/// All live sessions (admins can stop any with `DELETE /iptv/sessions/{id}`).
#[utoipa::path(get, path = "/admin/iptv/sessions", responses((status = 200, body = Vec<LiveSessionDto>)))]
pub async fn admin_live_sessions(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<LiveSessionDto>>> {
    Ok(Json(session_dtos(&st).await?))
}
