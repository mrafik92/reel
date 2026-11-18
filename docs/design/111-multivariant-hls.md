# Design: multi-variant HLS with lazy rungs (#111)

> **Status: approved 2026-11-17 (PR #178); the owner accepted every proposal in §16. ① (segment plan +
> keyframe alignment, #182) and ② (master playlist, lazy rung lifecycle, API, #198) are merged and deployed;
> ③ (web client) is in review; ④ isn't built.** Once ④ ships, the stable parts move into
> [ARCHITECTURE.md](../ARCHITECTURE.md) §3.1 and this note is trimmed to what's still useful.
>
> **Built differently from the text below (③):**
> - The estimate is fed from each hls.js fragment's own throughput (bytes over the time from first byte to
>   end, fragments ≥ 192 KB), not from `hls.bandwidthEstimate`: hls.js reports its seed (our estimate)
>   until it has enough samples, and timing from the first byte keeps a lazily started rung's ffmpeg
>   start-up from reading as a slow link.
> - No custom `fragLoadPolicy.shouldRetry`: hls.js 1.7.4 already never retries a 4xx, so a 429 goes
>   straight to its level switch. `preserveManualLevelOnError` keeps a pinned Original pinned when a
>   fragment fails, instead of falling back to ABR and starting a rung.
> - A rung choice counts as in the master only when a variant of exactly that size class exists (a rung
>   with that id, or an original that fits that box). Otherwise the play re-decides **without**
>   `adaptive`, i.e. a single-variant session at that rung, which is what #87 did. Capping at a smaller
>   rung would quietly give less than was asked.
> - The direct-play stall fallback caps the stored estimate at the file's bitrate before it re-decides,
>   so Q1's 1.5× rule picks remux + rungs; it runs once per play. The ranged probe during direct play runs
>   only when the estimate is older than 10 minutes.
> - Mock mode (`?mock=1`) serves a generated 3-variant H.264 HLS (a different test pattern per variant),
>   so switching can be checked in the browser without a server.
>
> **Built differently from the text below (②):**
> - The original's `BANDWIDTH` is always 1.7× its average bitrate; peaks from the index's byte positions
>   aren't read (the readers return times only).
> - Classic single-variant sessions keep today's one-line master without `CODECS` (adding it could make
>   hls.js drop a level whose string a browser rejects). Their variant stays `main`; a multi-variant session
>   names its primary `original`, or the top rung's class (`1080p`, `720p`, `480p`) when it only transcodes.
> - A stopped rung's finished segments are still served without starting its ffmpeg; only a segment it never
>   produced needs a new start (and admission).
> - A session counts against the caps per admitted variant that re-encodes something (an audio-only
>   transcode original counts while admitted). A variant is admitted from its first start until it is
>   stopped for being idle, not just while its process lives, so ffmpeg reaching the end of the file doesn't
>   free the slot early.
> - The variant ladder lives in `playback/src/variants.rs`; the decision rows of §4/§8 are its unit tests.

Epic #151, Wave 2. Replaces decision A6 ("one session = one rendition"). Builds on #87 (quality choice),
#112 item 1 (compressed playlists) and #113 part 1 (low-power encoders). Takes over what's left of #1
(the bandwidth estimator, minus PR #69's re-decide stepping).

## 1. The decision in one paragraph

An HLS session no longer serves one rendition. It serves a **master playlist with several variants**: the
**original** (video copied: remux or audio-only transcode) and one or more **rungs** (VAAPI encodes at
ladder sizes). All variants share **one segment plan**, and each rung forces an IDR keyframe at every
plan boundary, so the boundaries line up exactly. **hls.js** picks the variant per segment and switches
without a server round-trip. A variant's ffmpeg starts **lazily**, on the first request for one of its
segments (except the variant the player starts on, which starts at decide time as today). It is throttled
like today's sessions and **stopped when the player has left it for 30 s**. It counts against the transcode
caps only while its process exists.

## 2. Why this shape

From #102 (measured 2026-11-15):

- Every stall over Tailscale came from one fixed rendition riding a link that swung between ~1.6 and ~27 Mbit/s.
- Remuxing an HEVC MKV runs at **~107× realtime** with no GPU. A 1080p→720p VAAPI rung runs at **~9×**, and
  two concurrent rungs at ~4.5× each. "Original + rungs that only run while they're fetched" fits the
  3-transcode cap. A full live ladder (2.5×, 4.3× with low-power encoders) does not, and a pre-encoded
  ladder would need ~19 GPU-hours and ~330 GB.
- Switching per segment in the player is what YouTube does: start low, climb, and step down instead of
  stalling.

Constraint found while designing, in **hls.js 1.7.4** (the installed version; `^1.5.0` in
`packages/ui`): `AbrController.findBestLevel` **skips every level whose `codecSet` or `videoRange`
differs from the current one**. `codecSet` is the first four characters of the video and audio codec
(`avc1,mp4a`, `hvc1,mp4a`, …). That leads to three rules:

1. A rung uses the **same video codec family** as the original: HEVC originals get HEVC rungs, H.264
   originals get H.264 rungs.
2. **Audio is identical in every variant**: same action (copy or AAC encode), same codec, same channels.
3. All variants are SDR (today's browser profile tone-maps HDR anyway; #114 has to keep this rule).

## 3. When a session is multi-variant

The client opts in with `adaptive: true` on decide. Only hls.js players send it, because the player has to
switch levels itself and honour the quality choice (§9).

| Case | Variants |
|---|---|
| hls.js player, video can be copied (remux / audio transcode), keyframe index available | **original + rungs** (§4) |
| hls.js player, video must be encoded (HDR tone-map, unsupported codec, picture subtitles, user limit below the original) | **1–3 encoded rungs** (§8) |
| hls.js player, direct-playable file | direct play or HLS: see §7 and open question Q1 |
| Original is AV1 / VP9 / anything the server can't encode in the same family | **original only** (Q2) |
| No keyframe index yet (EVENT fallback playlist) | single variant, as today |
| Casts, native-HLS players (Safari, webOS, Tizen AVPlay), no `adaptive` flag | single variant, as today (Q6) |
| IPTV VOD (remote input, one provider connection, A26) | single variant, as today (Q5) |
| `REEL_ABR=0` (new kill switch) | single variant, as today |

## 4. The variant set (original + rungs)

Variant ids are the quality ids: `original`, `1080p`, `720p`, `480p`. They're also the URL path
segment (`/stream/{sid}/720p/index.m3u8`, replacing today's fixed `main/`).

**Rungs below a copyable original.** Take every ladder rung (1080p/8M, 720p/4M, 480p/2M from
`quality.rs`) that never upscales (rung height ≤ source height) and whose total bitrate is **≤ 0.7× the
original's average bitrate**, at most three. They cost nothing until fetched, so listing all of them is
cheap (Q4). Then:

- **Codec:** the original's family. HEVC rungs use the HEVC ladder (§13, Q3), are tagged `hvc1` like the
  copied original, and keep the source's bit depth (Main10 stays Main10, so the SourceBuffer never needs
  a profile change). If the selected backend can't encode that family in hardware (no VAAPI HEVC,
  software fallback), there are no rungs.
- **Audio:** the original's audio action, copied verbatim (copy AAC/Opus/FLAC, or the same AAC encode).
- **User bitrate limit** (`user_limits.max_bitrate_kbps`): variants above it aren't listed. 480p always
  stays, as today. If the original is over the limit, the session is transcode-only (§8).
- **Room check at decide:** rungs are listed only if a transcode slot is free right now (global and
  per-user, counting live TV, A35). Otherwise the master lists only the original. The check is repeated at
  lazy start (§6).

Examples, Chrome on the owner's Mac (decodes HEVC):

| Title | Original | Rungs |
|---|---|---|
| Meet Joe Black /item/86 (HEVC 10-bit 1080p, 6.5 Mbit/s, AAC) | remux, 6.5 M | 720p HEVC, 480p HEVC |
| Spider-Man /item/107 (H.264 1080p, 24.7 Mbit/s, EAC3) | audio transcode (AAC), 24.7 M | 1080p, 720p, 480p H.264, all with the same AAC |
| Paranormal Activity /item/98 (H.264 1080p, 2.7 Mbit/s, AAC, MP4) | direct play or remux (§7) | none (480p is not ≤ 0.7× of 2.7 M) |

## 5. Master playlist

Written in ascending bandwidth order (hls.js sorts the same way), with an exact `CODECS` attribute
(hls.js drops levels that `MediaSource.isTypeSupported` rejects) and both bandwidth figures, because hls.js
ABR uses `AVERAGE-BANDWIDTH` while the buffer is healthy and `BANDWIDTH` (peak) when it's low:

```m3u8
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-STREAM-INF:BANDWIDTH=1850000,AVERAGE-BANDWIDTH=1560000,RESOLUTION=854x462,CODECS="hvc1.2.4.L90.B0,mp4a.40.2"
480p/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3600000,AVERAGE-BANDWIDTH=3000000,RESOLUTION=1280x692,CODECS="hvc1.2.4.L93.B0,mp4a.40.2"
720p/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=11200000,AVERAGE-BANDWIDTH=6500000,RESOLUTION=1920x1038,CODECS="hvc1.2.4.L120.B0,mp4a.40.2"
original/index.m3u8
```

- **Rungs:** `BANDWIDTH` = video `maxrate` (b + b/5) + audio; `AVERAGE-BANDWIDTH` = b + audio.
- **Original:** `AVERAGE-BANDWIDTH` = the file's average bitrate. `BANDWIDTH` = the peak over the plan's
  segments when the keyframe index carries byte positions (MKV Cues have cluster positions; MP4 has
  `stco`/`stsz`), otherwise 1.7× the average. A VBR film's peaks run well above its average (The Remarried
  Empress: 5.9 Mbit/s average, 11–13 Mbit/s at 2:44–3:16).
- **`CODECS`** is built from the probed `streams.profile`/`level`/`bit_depth` for the copied original and
  from the encoder settings for rungs: `avc1.PPCCLL`, `hvc1.{1|2}.{4|6}.L{level×30}.B0`, `mp4a.40.2`,
  `opus`, `fLaC`, `ac-3`, `ec-3`. Unit tests cover the mapping, and the Chrome e2e test asserts
  `isTypeSupported` for every emitted string.
- No `FRAME-RATE` (rungs keep the source rate) and no `VIDEO-RANGE` (SDR is the default).
- Single-variant sessions keep today's one-line master (now with `CODECS` too).

Every variant playlist is the same VOD playlist (`build_plan_playlist`) with identical `#EXTINF` lists. The
signing (`stream_guard` → `sign_playlist`) already covers relative variant URIs.

## 6. Shared segment plan and keyframe alignment

**One plan per session**, computed at decide as today and shared by every variant:

- **Copyable original:** `SegmentPlan::from_keyframes` from the source's real keyframes (cache → MKV
  Cues/MP4 `stss` → background ffprobe). Segments are ≥ 4 s, cut at the source's own keyframes (variable
  length).
- **Transcode-only:** `SegmentPlan::fixed` (4 s grid).

**Original (copy):** unchanged. ffmpeg writes one file per source GOP, and the session assembles and retimes
them per plan segment (`assemble_segment`).

**Rungs:** encode with forced IDR frames at exactly the plan's segment starts and nowhere else, and cut a
file at every keyframe:

```
-ss {plan.start(n)}                         # accurate seek (decoding), no margin needed for encodes
-force_key_frames {start(n+1)-ε},{start(n+2)-ε},…   # boundaries after the restart segment, ε = 1 ms
-g 1000000 -keyint_min 1000000 -sc_threshold 0    # no periodic or scene-cut keyframes (x264: also keyint/scenecut)
-f hls -hls_time 0.001 -start_number n -hls_segment_filename seg_%05d.m4s
```

Because the plan's starts *are* source keyframe timestamps, and a rung doesn't change the frame rate, the
first frame at or after each forced time is the frame the original's segment starts with. The rung's
segment `n` therefore covers exactly the same frames as the original's segment `n`. That holds even
though the original's keyframes are wherever the source put them, because the plan is made from them.

Measured locally (ffmpeg 9, libx264 standing in for VAAPI; 30 s source with keyframes at 0, 3.208, 7.917,
12.5, 16.125, 21, 25.333 and 28.625 s; plan = 0, 7.917, 12.5, 21, 25.333):

| Run | Rung segment lengths | Plan segment lengths |
|---|---|---|
| from 0 | 7.917, 4.583, 8.500, 4.333, 4.667 | same |
| restart at segment 2 (`-ss 12.5`), only future times passed | 8.500, 4.333, 4.667 | same |
| restart at segment 2 with the **full** list | 0.042, 0.042, 8.417, … | broken: ffmpeg forces one keyframe per already-past time |

So a restart must pass **only future times**. It also leaves out the restart boundary itself: the
encoder's first frame is always an IDR, and ffmpeg ≤ 6's accurate seek starts on the frame *covering*
the seek time (79.997 s for `-ss 80` at 23.976 fps), so listing 79.999 forced the next frame too and
made a 1-frame segment (seen in CI on Ubuntu's ffmpeg and on CT 113's 5.1).

**Checked on CT 113** (ffmpeg 5.1, iHD; 2026-11-17, nobody watching, scratch dir removed): with
`-g 100000` and the list, `h264_vaapi -low_power 1`, `hevc_vaapi -low_power 1` and libx264 each wrote
one file per plan segment with exactly one keyframe, on a source-keyframe plan (from 0 and restarted
at segment 5) and on the fixed grid restarted at `-ss 80`. The copy side assembled the
same window from GOPs 12.5–16.125 and 16.125–21, which is the same 8.5 s.

**Fixed grid fix (also helps single-variant sessions):** today's
`expr:if(isnan(prev_forced_t),gte(t,0),gte(t,prev_forced_t+4))` is relative, so at 23.976 fps every
keyframe lands 4.004 s after the previous one: **+0.4 s after 100 segments** (measured: segment 100 at
400.400 s). Two rungs restarted at different points would disagree. An expression can't fix it: in
`-force_key_frames expr:…`, `t` counts from the run's first output frame, not the source timestamp
(after `-ss 12`, `gte(t,12+n_forced*4)` put keyframes at 12, 24, 28). An explicit **time list is
absolute** (source timestamps, also with `-ss` + `-copyts`), so **every encode on a plan, fixed or
keyframe, uses the list** and cuts a file at every keyframe (`-g 100000`, `hls_time 0.001`). The
muxer's own `hls_time 4` grid can't be kept for fixed plans: it counts from the run's first frame,
which an accurate seek puts up to a frame after the boundary, so it skipped the absolute keyframe
and merged two segments (caught by the existing `segments_valid` test). For a very long file the list
is capped near 100 KB of argv (Linux `MAX_ARG_STRLEN` is 128 KB, about 10 h of 4 s segments). Past it,
the segment check rejects the next segment and the variant restarts there.

**Timeline normalisation** is unchanged and per variant: each variant keeps its own `epoch` and
`run_delta`, and every segment is retimed so its first video sample sits exactly at `plan.start(n)`
(`normalize_segment` for rungs, `assemble_segment` for the original). **New check** (rungs on a
keyframe plan only, so variable-frame-rate transcodes on the fixed grid can't be rejected): a rung
segment's start and duration must match the plan within 60 ms (under one frame at ≥ 20 fps). A stray keyframe (an encoder that ignores `-g`)
is caught as an invalid segment and makes that variant's ffmpeg restart, instead of quietly shifting
every later boundary.

## 7. Direct play versus the original variant

hls.js can't use a progressive MP4 as an HLS variant, so a direct-playable file has no ABR unless it's
remuxed. **Proposal (Q1):** with `adaptive: true` and quality Auto,

- direct play when the stored estimate is ≥ 1.5× the file's average bitrate (comfortably fast);
- otherwise a remux original plus rungs. This also fixes the slow start of MP4s with a trailing `moov`
  (#112 item 3: 8.9 s → a remux first segment in < 1 s);
- with no estimate (the first play on a device): remux plus rungs, starting low.

Original and 1080p/720p/480p choices behave as today for direct-playable files. #1's remaining fallback
stays for direct play: two stalls in 30 s re-decide from the playhead as HLS (same `device_id`, so the old
session is replaced).

## 8. Files that only transcode

The **top rung** is what decide picks today with no bandwidth hint (the user limit still applies), for
example 1080p tone-mapped for a 4K DV file, or source size for an unsupported codec. **Lower rungs** are
the ladder rungs below it at ≤ 0.7× its bitrate. They're H.264 for now (#113 part 2 picks HEVC for
HEVC-capable devices later). They share the fixed 4 s plan, the absolute keyframe grid, the same audio and
the same burn-in.

**GPU budget:** lower rungs are listed only if **two** transcode slots are free at decide (one for the
active rung, one for a switch). Otherwise there's **one rung**, chosen from the estimate as today's
bandwidth cap would. That's the Done-when's "1 variant when only a transcode is possible and the GPU
budget is full".

## 9. Variant lifecycle

Each variant has its own `Proc` (child, start segment, paused flag, `started`), `epoch`, `run_delta`, HW
backend and scratch subdirectory (`transcode/{sid}/{variant}/`). The session keeps the plan, decision,
input, staged burn-in file, metadata and the session-level idle reaper.

| State | Entered when | Counts against transcode caps | Holds an ffmpeg process slot |
|---|---|---|---|
| **idle** | session created (variants other than the start variant), or stopped | no | no |
| **running** | prestart at decide (the start variant), or the first request for one of its segments / its init | if it encodes (video or audio) | yes |
| **paused** | the read-ahead throttle (`max_ahead_segments` 20) SIGSTOPs it, as today | yes | yes |
| → idle | **no request for this variant for `variant_idle_timeout` (30 s)**: kill ffmpeg, release the slots, keep its finished segments for a quick return | | |

- **Lazy start = admission.** Starting a variant takes the transcode admission lock (A35), re-checks the
  global and per-user caps (live TV included), takes a process permit, then spawns at the requested
  segment. If either is full, that request gets **429** and the variant stays idle. The client's
  `fragLoadPolicy.shouldRetry` doesn't retry 429, so hls.js's error controller moves to another level
  (penalty box). The player keeps playing what it has. A 503 (segment not ready) keeps today's retries.
- **Counting** moves from "session mode is Transcode/AudioTranscode" (for the session's whole life) to
  "running/paused variants that encode", per session, summed. A remux-only original never counted and
  still doesn't. An audio-transcode original now stops counting once it's idle-stopped.
- **The decide-time limit check** in `decide_playback` uses the same count. The prestarted variant is
  admitted under the lock, as `create_session` does today.
- **Seek and restart logic** is today's `segment_path`, per variant: a request outside
  `[start, head + lookahead]` restarts *that* variant's ffmpeg. Other variants are untouched.
- **HW fallback** (`hw_startup_grace`, low power → regular → software) is per variant. Degradation is
  still recorded server-wide, as today.
- **Disk:** the per-session budget (4 GB) covers all variants. Trimming runs per variant behind that
  variant's last request. The global budget and eviction are unchanged.
- The **session idle reaper** is unchanged: any request on any variant, or a progress ping, keeps the
  session alive. `remove` kills every variant.

## 10. Far seeks

hls.js picks the level for the first fragment after a seek with an empty buffer. It then uses the peak
`BANDWIDTH` and `maxStarvationDelay` (4 s), so on a weak link it fetches a lower rung and on the LAN it
stays on the original. The server restarts only the variant that's asked for, at the plan segment of the
seek target: a 720p rung's segment after a far seek was ready in 1.05–1.23 s and a 4K tone-map in 2.67 s
(#102). A remux restart has no encode, so it should be faster; ① measures it. Then hls.js climbs
as fragments arrive, starting the original lazily on the first up-switch. The variant it left goes idle
after 30 s.

## 11. Quality choice (#87) mapped onto variants

For `adaptive: true` sessions, the server lists every variant that §4/§8 allow, **whatever `quality` was
asked**. `quality` picks the **start variant** and keeps its #87 meaning for the explanations. The player
applies the choice with hls.js, so switching between choices needs **no re-decide** while the target
variant is in the master:

| Choice | hls.js | Notes |
|---|---|---|
| **Auto** | `currentLevel = -1` (ABR over all levels) | start level from the estimate (§12) |
| **Original** | pin `currentLevel` to `original` (manual level) | no rung is ever fetched, so none starts. If there's no `original` variant, #87's "Play transcoded" prompt stays |
| **1080p / 720p / 480p** | `autoLevelCapping` = highest level with height ≤ the choice | ABR below the cap; "never upscales" holds because rungs are never above the source |

A choice whose variant isn't in the master (for example a rung left out because the GPU was full)
re-decides from the playhead, as today. Single-variant sessions keep today's behaviour (the server picks
the rendition from `quality`; switching re-decides). The per-device default
(`localStorage reel.playback.quality`) is read the same way.

## 12. Bandwidth estimate (#1) and hls.js start

Salvaged from PR #69 (`origin/1-adaptive-quality-client`, `lib/abr.ts`): the per-server stored estimate
(`reel.bw.{server}` in `localStorage`, 3-day max age, drops weighted more than recoveries) and the ranged
probe for direct play. Its `AbrController` re-decide stepping is dropped.

- **Feeding:** on `FRAG_LOADED`, record `hls.bandwidthEstimate` (hls.js's own EWMA). For direct play, use
  the ranged probe as in #69.
- **Decide** sends `bandwidth_kbps` (the estimate; it's **not a cap**, unlike `max_bitrate_kbps`). The
  server picks `start_variant`: the highest variant whose peak `BANDWIDTH` ≤ 0.8 × estimate, or the lowest
  variant with no estimate ("start low, then climb"). It prestarts that variant and returns the variant
  list and `start_variant`.
- **hls.js config** (`packages/ui` `hlsConfig`, VOD only):
  - `abrEwmaDefaultEstimate` = estimate (bps) when there is one;
  - `testBandwidth: false` (otherwise hls.js downloads a throwaway fragment of the lowest level, which
    would start a rung on every play);
  - `autoStartLoad: false`; on `MANIFEST_PARSED`, set `hls.startLevel` to the level whose URL is
    `start_variant`, apply the §11 pin/cap, then `startLoad(startPosition)`. Levels are matched by URL
    path, not index, because hls.js may drop levels it can't play.
- **Stream info and Settings:** stream info shows the playing level (`LEVEL_SWITCHED`) and the estimate.
  Settings → Playback shows the measured speed next to the choice (#87's extra Done-when).
- The estimate is per device and per server name. `reel.rafikii.co` is split-horizon, so a LAN estimate
  is used on the first remote play. hls.js's abandon rule drops a level within one fragment, which bounds
  the cost to one slow segment.

## 13. API, session, DB, openapi

**Decide** (`POST /playback/{file_id}/decide`), new optional fields:
- `adaptive: bool`: the player switches variants itself; return a multi-variant master when possible.
- `bandwidth_kbps: u32`: measured throughput; chooses the start variant. It doesn't cap anything.

**DecideResponse**, new fields:
- `variants: [VariantOption]`, in master order: `{ id: Quality, label, width, height, bandwidth_kbps (peak),
  average_kbps, codecs, transcode: bool }`. Single-variant sessions return one entry.
- `start_variant: Quality`.
- `decision`, `quality` and `bitrate_kbps` keep describing the **top** variant (the original, or the top
  rung of a transcode-only session), so #87's "Original needs a transcode" logic is unchanged.

**`SessionInfo`** (`/admin/sessions`), new fields:
- `variants: [{ id, label, encoder?, state: idle|running|paused, last_request_s? }]`;
- `delivering: Quality?`: the variant of the most recent segment request.
- `encoder`/`bitrate_kbps` follow `delivering`.

**Session internals:** `Session` keeps the shared parts; a new `Variant` struct holds the per-variant
state (§9). `VARIANT = "main"` goes away; the router already has `/stream/{sid}/{variant}/{file}`.

**Config:**
- `REEL_ABR` (default on; `0` = single variant everywhere, a rollback lever without a rebuild);
- `REEL_ABR_VARIANT_IDLE_SECS` (default 30);
- documented in OPERATIONS.md in PR ②.

**DB:** no migration. The `playback_sessions` row keeps the top variant's decision and mode, as today.
Which variant is delivered is live state, shown only in Now playing.

**openapi:** `server/openapi.json` regenerated in PR ②. The client types are added by hand in `api/types.ts`
like #87's quality fields (a full `gen:api` pulls in unrelated drift), unless that drift is fixed first.

**Admin → Now playing:** one line per session, e.g. "Delivering **720p** · `hevc_vaapi (low power)` · 3.0 Mbps",
then the variants with their state ("Original idle · 480p paused"). Single-variant sessions look as today.

**HEVC ladder for rungs** (Q3): 1080p 5 M, 720p 2.8 M, 480p 1.4 M, about 0.7× of H.264, per #113's
measurements. The H.264 ladder is unchanged.

## 14. PR stack

Each PR says `Refs #111` (③ also `Refs #1`), is green on its own, and stops for the coordinator to merge and
deploy. Each is stacked on the previous branch, or rebased on `main` once it merges.

| PR | Scope | User-visible change |
|---|---|---|
| **① Segment plan + keyframe alignment** | `Session` → `Session` + `Variant` refactor (still one variant); plan-driven `-force_key_frames` (explicit list for keyframe plans, absolute grid for fixed plans), `-g`/x264 keyint so only forced keyframes exist; a rung on a keyframe plan writes `seg_n` directly; duration check in the validator; `CODECS` builder | None (the fixed grid stops drifting). Checked on CT 113 that `h264_vaapi`/`hevc_vaapi` low-power encodes honour forced IDRs with a huge GOP (read-only, scratch dir, nobody watching) |
| **② Master playlist + lazy rung lifecycle + API** | variant set (§4, §8), master (§5), lazy start/idle stop/counting (§9), per-variant seeks, 429 on refused lazy start, `adaptive`/`bandwidth_kbps`/`variants`/`start_variant`, `SessionInfo.variants`/`delivering`, `REEL_ABR`, openapi, mock data, OPERATIONS | Nothing for users yet (no client sends `adaptive`) |
| **③ Client: hls.js ABR + estimator** (closes #1's remaining scope) | `lib/abr.ts` estimator (salvaged), hls.js seeding and start level (§12), 429 no-retry, quality mapping (§11) incl. the player picker switching locally, stream info level + estimate, Settings measured speed, direct-play stall → HLS re-decide, direct-vs-remux rule (§7) | Auto adapts per segment. `Player.tsx` changes stay small: load options and the picker handler; level logic lives in `lib/abr.ts` and the adapter |
| **④ Now playing + docs** | Admin → Now playing variant display; FEATURES, STATUS, ARCHITECTURE §3.1 flow, this note folded in; live numbers | Admins see which variant is delivered |

## 15. Test plan (from the ticket's Done-when)

**Server (real ffmpeg fixtures, `crates/playback/tests`):**
- Alignment: a 150 s MKV with an irregular GOP, B-frames and 23.976 fps. For every segment `n`, the
  original's and each rung's first video `tfdt` are equal and their durations match the plan. This holds
  after independent restarts of each variant at random segments too (extends `timeline.rs`).
- Keyframe args: unit tests that a restart at `n` passes only `start(n..)`, that the argv is capped, and
  that the fixed grid is absolute.
- Master: variants and their `BANDWIDTH`/`AVERAGE-BANDWIDTH`/`RESOLUTION`/`CODECS` for a copyable H.264 file
  and an HEVC file. One variant for a transcode-only file when the global cap leaves fewer than two free
  slots. Only the original when no slot is free.
- Lazy lifecycle: no rung ffmpeg before its first segment request; a request starts it; with
  no further requests it's paused and then stopped after `variant_idle_timeout` (shortened in tests); the
  transcode counts include it only while it runs; a lazy start with the cap full returns 429 and doesn't
  spawn.
- `chrome_e2e.rs` (ignored by default, run locally): hls.js over a 2-variant master; force `nextLevel`
  up and down every two segments for 60 s, then seek. No `bufferAppendError`, one contiguous buffered
  range, playback advances. Every emitted `CODECS` passes `MediaSource.isTypeSupported`.

**Client:** typecheck/build; Playwright in mock mode (variants in the mock decide response) and live,
desktop and **1920×1080 TV mode with arrow keys** (picker: Auto/Original/720p switch locally; stream info
shows the level).

**Live (CT 113, after deploy, nobody watching):**
- Chrome over Tailscale with CDP throttling at 3.5 Mbit/s on Meet Joe Black (/item/86): 5 min with no stall
  > 1 s after the first 10 s. With throttling removed, the original variant within 30 s.
- On the LAN: first frame ≤ 2 s from pressing Play, far seek ≤ 3 s, delivered variant = original for a file
  under the link's capacity.
- Admin → Now playing shows the delivered variant. `ps` on CT 113 shows the rung's ffmpeg only while it's
  fetched.

## 16. Questions the owner decided (2026-11-17: every proposal accepted)

| # | Question | Decision |
|---|---|---|
| Q1 | Direct-playable files under Auto: keep direct play (no ABR) or remux to get ABR? | Direct play when the stored estimate ≥ 1.5× the file's bitrate, else remux + rungs (and remux on the first play with no estimate). Costs one remux process (~107× realtime, no GPU) |
| Q2 | AV1/VP9 originals (14 AV1 files): the N100 can't encode an AV1 rung, and hls.js won't auto-switch to an H.264 one | Original only, plus #1's stall → re-decide fallback. Alternative: list H.264 rungs and have the client force a cross-codec switch (Chrome supports `changeType`), which means custom ABR code |
| Q3 | HEVC rung bitrates | 1080p 5 M / 720p 2.8 M / 480p 1.4 M (about 0.7× H.264). **VMAF check (2026-11-17)**: 60 s of Meet Joe Black (10-bit x265 1080p) from 30:00, low-power VAAPI on CT 113, each encode scaled back to 1920×1040 and scored against the source. HEVC vs H.264 per rung: 1080p 78.9 vs 79.3 (5.5 vs 9.0 Mbit/s measured), 720p 73.6 vs 73.9 (3.1 vs 4.5), 480p 64.5 vs 64.5 (1.5 vs 2.3). Same picture within 0.4 VMAF at 61–68 % of the bitrate |
| Q4 | How many rungs to list | All qualifying (≤ 3), since idle rungs cost nothing. Alternative: cap at 2 (highest below the original + 480p) |
| Q5 | IPTV VOD stays single-variant (two ffmpegs would need two provider connections; the plan allows one) | Yes. Later option: one ffmpeg with two outputs (copy + encode) on the single connection, but then the rung always runs |
| Q6 | Safari/webOS/Tizen native players and casts: their own ABR could use a multi-variant master, but they can't be pinned to Original/720p | Single variant for now; opt Safari in later once ② is proven |
| Q7 | GPU budget with Plex: the caps count only Reel's processes (`REEL_MAX_TRANSCODES=3`). A switch can run two rungs of one session for up to 30 s | Keep 3. Each lazy start re-checks the cap, so it's never exceeded, only used more evenly |

## 17. Risks

- **iHD low-power encoder and forced IDRs with a huge GOP:** production already depends on forced
  keyframes with low-power encoders (the fixed-grid expression). The new parts are the explicit list and
  `-g`. ① checks it on CT 113 before ② starts, and the duration check in §6 catches a misbehaving encoder
  at runtime.
- **Main10 ↔ Main switching:** avoided by keeping the source bit depth in HEVC rungs. hls.js would call
  `changeType` otherwise, which Chrome supports.
- **Up-switch latency:** the first segment of a variant that was idle-stopped waits for ffmpeg (a rung
  ~1–1.2 s per #102; a remux less, measured in ①). hls.js only switches up with a healthy buffer, so this doesn't stall playback.
- **Scratch disk:** up to two variants' segments per session, within the existing 4 GB/session and 20 GB
  total budgets.
- **Split-horizon estimate** (§12): one slow first segment on the first remote play after LAN use.
