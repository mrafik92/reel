/**
 * Live TV fixtures for `?mock=1`: ~60 channels across groups, a deterministic EPG, a playable
 * stream (the dev-server test pattern) and a simulated hub. `?iptv=empty` shows the first-run state.
 */
import { ApiError } from "../http";
import type {
  AdminChannel, ChannelMetrics, EpgGrid, HubStatus, IptvApi, IptvChannel, LiveSession, NowNext, Program,
  SourceBody, SourceDto, SourceTest,
} from "../iptv";
import { SAMPLE_VIDEO } from "./fixtures";

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const latency = (lo = 90, hi = 260) => sleep(lo + Math.random() * (hi - lo));
const nowS = () => Math.floor(Date.now() / 1000);

/** `?iptv=empty` (no provider) / `?iptv=big` (11,950 channels, 175 groups, no guide) — sticky per tab. */
function mode(): "empty" | "big" | "normal" {
  try {
    const q = new URLSearchParams(location.search).get("iptv");
    if (q === "empty" || q === "big") sessionStorage.setItem("reel.iptv", q);
    else if (q) sessionStorage.removeItem("reel.iptv");
    const v = sessionStorage.getItem("reel.iptv");
    return v === "empty" || v === "big" ? v : "normal";
  } catch {
    return "normal";
  }
}

const BIG_WORDS = ["Africa", "Arabic", "Sports", "Kids", "Movies", "News", "Music", "Docs", "Series", "Premium", "Local", "Religious", "Comedy", "Classic", "World"];
const BIG_LANDS = ["UK", "France", "Germany", "Egypt", "Saudi", "Morocco", "Turkey", "India", "USA", "Canada", "Spain", "Italy"];

/** Shape of a real Xtream provider: thousands of channels, many groups, no favourites, no EPG. */
function buildBig(): Chan[] {
  const r = rng(42);
  const out: Chan[] = [];
  let id = 1;
  for (let g = 0; g < 175; g++) {
    const name = `${BIG_WORDS[g % BIG_WORDS.length]} ${BIG_LANDS[Math.floor(g / BIG_WORDS.length) % BIG_LANDS.length]}${g >= 180 ? " " + g : ""} - مجموعة ${g + 1}`;
    const n = g === 174 ? 11950 - (id - 1) : 20 + Math.floor(r() * 90);
    for (let i = 0; i < n && id <= 11950; i++) {
      const cname = `${name.split(" - ")[0]} ${i + 1}${r() > 0.7 ? " FHD" : r() > 0.5 ? " HD" : ""}`;
      out.push({
        ch: { id, name: cname, number: id, group: name, logo_url: id % 3 ? logo(cname, id) : null, favorite: false, has_epg: false, catchup: false },
        admin: { id, source_id: 1, name: cname, number: id, group: name, enabled: true, missing: false, catchup: false, aliases: [], epg_channel_id: null, epg_manual: false, tvg_id: null },
      });
      id++;
    }
  }
  return out;
}

/** mulberry32 — tiny deterministic PRNG so the guide is stable across calls. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface Def {
  name: string;
  group: string;
}

const GROUPS: Record<string, string[]> = {
  News: ["Reel News 24", "Harbour Report", "Global Wire", "Market Watch", "Parliament Live", "Weather Now", "Northern Bulletin"],
  Sports: ["Arena One", "Arena Two", "Pitchside", "Motor Grid", "Court & Net", "Fight Night", "Peloton TV", "Golf Links"],
  Movies: ["Cine Classics", "Midnight Movies", "Premiere Plus", "Noir Channel", "Family Films", "Action Max", "Arthouse"],
  Entertainment: ["Channel Ivory", "Lime TV", "Sitcom Central", "Reality One", "Late Show Network", "Quiz Hour", "Comedy Cellar", "Drama Lane"],
  Kids: ["Little Reel", "Cartoon Harbour", "Junior Lab", "Bedtime Stories", "Toon Town"],
  Documentary: ["Wild Planet", "History Vault", "Science Deep", "Ocean Blue", "Engineered", "True Crime Files"],
  Music: ["Hits Live", "Jazz Lounge", "Rock Arena", "Classical FM TV", "Club Beats"],
  International: ["France Directe", "Deutsche Welle Mix", "Tokyo Today", "Roma Uno", "Canal Ibérico", "Nordic Nine", "Seoul Wave", "Rio Ao Vivo"],
};

const TITLES: Record<string, string[]> = {
  News: ["The Morning Edition", "World Tonight", "Headlines", "The Briefing", "Business Desk", "Politics Live", "Newsnight", "Weekend Wrap", "Breaking: Live Coverage"],
  Sports: ["Matchday Live", "Premier Highlights", "Grand Prix Qualifying", "Tennis: Quarter Final", "The Fight Card", "Tour Stage 12", "Golf: Final Round", "Transfer Talk", "Football Tonight"],
  Movies: ["The Long Goodbye", "Harbour Lights", "A Quiet Storm", "Night Train to Lisbon", "The Glass Key", "Paper Moon", "Northern Star", "The Last Signal", "Blue Hour"],
  Entertainment: ["Friends of the Show", "Kitchen Wars", "The Big Quiz", "Late Night with Juno", "Home Rescue", "Stand-Up Sessions", "The Apartment", "Love Island Bay", "Antique Hunters"],
  Kids: ["Pip & Pebble", "Rocket Club", "Ocean Pals", "Little Builders", "Story Time", "Dino Detectives", "Paint Party"],
  Documentary: ["Planet of Ice", "Secrets of Rome", "The Deep", "Megastructures", "Inside the Hive", "Cold Case Files", "Rivers of Africa", "Cosmos Rising"],
  Music: ["Top 40 Countdown", "Live at the Lounge", "Rock Legends", "Symphony Hall", "Club Mix", "Unplugged", "Video Hits"],
  International: ["Journal du Soir", "Tagesschau Extra", "Tokyo Street Food", "Calcio Live", "Telediario", "Nordic Noir", "K-Drama Hour", "Novela das Nove"],
};

const BLURBS = [
  "Live coverage, interviews and analysis from around the world.",
  "A brand-new episode: tempers flare as the deadline approaches and old rivals return.",
  "The classic, restored in high definition. A slow-burning story of loyalty and loss.",
  "Highlights, goals and post-match reaction from the day's biggest fixtures.",
  "An extraordinary journey to places few people have ever seen.",
  "The team takes on its toughest challenge yet — and nobody is safe.",
];

const PALETTE = ["#c6f432", "#ff8a3d", "#5ad1ff", "#ff5a7a", "#b48cff", "#ffd23f", "#3dffb4", "#f3eee4"];

function logo(name: string, i: number): string {
  const words = name.replace(/[^A-Za-z0-9 ]/g, "").split(/\s+/).filter(Boolean);
  const initials = (words.length > 1 ? words[0]!.charAt(0) + words[1]!.charAt(0) : name.slice(0, 2)).toUpperCase();
  const c = PALETTE[i % PALETTE.length]!;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 64">` +
    `<rect width="96" height="64" rx="10" fill="#171a23"/>` +
    `<rect x="8" y="52" width="24" height="4" rx="2" fill="${c}"/>` +
    `<text x="48" y="40" font-family="Arial Black,Arial,sans-serif" font-weight="900" font-size="26" text-anchor="middle" fill="${c}" letter-spacing="-1">${initials}</text>` +
    `</svg>`;
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}

interface Chan {
  ch: IptvChannel;
  admin: AdminChannel;
  /** Programme schedule, generated lazily. */
  sched?: Program[];
}

function buildChannels(): Chan[] {
  const out: Chan[] = [];
  let id = 101;
  let num = 1;
  let i = 0;
  for (const g in GROUPS) {
    for (const name of GROUPS[g]!) {
      const def: Def = { name, group: g };
      const tvg = name.toLowerCase().replace(/[^a-z0-9]+/g, "") + ".mock";
      const hasEpg = !(i % 23 === 22); // a few channels without guide data
      out.push({
        ch: { id, name: def.name, number: num, group: def.group, logo_url: logo(def.name, i), favorite: i === 0 || i === 8 || i === 22 || i === 30, has_epg: hasEpg, catchup: i % 5 === 0 },
        admin: { id, source_id: i % 9 === 8 ? 2 : 1, name: def.name, number: num, group: def.group, enabled: true, missing: false, catchup: i % 5 === 0, aliases: [], epg_channel_id: hasEpg ? tvg : null, epg_manual: false, tvg_id: hasEpg ? tvg : null },
      });
      id++;
      num++;
      i++;
    }
    num = Math.ceil(num / 10) * 10 + 1; // groups start on round numbers: 1, 11, 21…
  }
  // One provider-side disabled channel for the admin view.
  out[out.length - 1]!.admin.enabled = false;
  return out;
}

/** Deterministic schedule from yesterday 00:00 to +3 days. */
function schedule(c: Chan): Program[] {
  if (c.sched) return c.sched;
  const r = rng(c.ch.id * 7919);
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  let t = Math.floor(day.getTime() / 1000) - 86400;
  const end = t + 4 * 86400;
  const g = c.ch.group ?? "Entertainment";
  const titles = TITLES[g] ?? TITLES.Entertainment!;
  const lens = g === "Movies" ? [90, 105, 120, 95] : g === "Sports" ? [60, 120, 150, 30] : g === "News" ? [30, 30, 60, 15] : g === "Kids" ? [15, 30, 30, 25] : [30, 60, 60, 45];
  const out: Program[] = [];
  // Stagger first start so the grid isn't one big aligned block.
  t += [0, 15, 30, 45][Math.floor(r() * 4)]! * 60;
  let ep = 1 + Math.floor(r() * 20);
  while (t < end) {
    const mins = lens[Math.floor(r() * lens.length)]!;
    const title = titles[Math.floor(r() * titles.length)]!;
    out.push({
      start: t,
      stop: t + mins * 60,
      title,
      sub_title: r() > 0.55 ? `Episode ${ep++}` : null,
      category: g,
      episode_num: null,
      description: BLURBS[Math.floor(r() * BLURBS.length)]!,
      icon: null,
    });
    t += mins * 60;
  }
  c.sched = out;
  return out;
}

const compact = (p: Program): Program => ({ start: p.start, stop: p.stop, title: p.title, sub_title: p.sub_title, category: p.category });

export function createMockIptv(): IptvApi {
  const m = mode();
  const empty = m === "empty";
  const chans: Chan[] = empty ? [] : m === "big" ? buildBig() : buildChannels();
  const byId = new Map(chans.map((c) => [c.ch.id, c]));
  const need = (id: number) => {
    const c = byId.get(id);
    if (!c) throw new ApiError(404, "Channel not found");
    return c;
  };
  const now = nowS();
  let nextSource = 3;
  const sources: SourceDto[] = empty
    ? []
    : [
        {
          id: 1, name: "Harbour IPTV", kind: "xtream", url: "http://line.harbour-iptv.example:8080", username: "rafik", password_set: true,
          enabled: true, channel_count: chans.filter((c) => c.admin.source_id === 1).length, refreshing: false, open_connections: 1,
          max_connections: 2, last_refresh_at: now - 3 * 3600, last_epg_at: now - 3 * 3600, last_error: null, epg_url: null,
          detected_epg_url: "http://line.harbour-iptv.example:8080/xmltv.php", refresh_interval: null, user_agent: null,
        },
        {
          id: 2, name: "Free-to-air M3U", kind: "m3u", url: "https://example.org/playlists/fta.m3u8", username: null, password_set: false,
          enabled: true, channel_count: chans.filter((c) => c.admin.source_id === 2).length, refreshing: false, open_connections: 0,
          max_connections: null, last_refresh_at: now - 26 * 3600, last_epg_at: now - 26 * 3600, last_error: "EPG: 3 channels had no match",
          epg_url: "https://example.org/epg/fta.xml.gz", detected_epg_url: null, refresh_interval: 720, user_agent: null,
        },
      ];

  // ---- sessions + hub simulation -------------------------------------------------
  const sessions = new Map<string, LiveSession>();
  // Big mode: someone (you, on another screen) is already watching channel 3 on a 1-connection plan.
  const busyChannel = 3;
  let bigFree = false;
  if (m === "big") {
    sessions.set("tv-1", { id: "tv-1", user_id: 7, username: "sam", channel_id: busyChannel, channel_name: chans[busyChannel - 1]!.ch.name, mode: "passthrough", started_at: nowS() - 900, idle_s: 0, connections: 1, hls: null });
  }
  const warm = new Map<number, number>(); // channel → since
  let sid = 1;
  const metrics = (chId: number): ChannelMetrics => {
    const c = byId.get(chId)!;
    const viewers = Array.from(sessions.values()).filter((s) => s.channel_id === chId).length;
    const t = nowS();
    return {
      channel_id: chId, name: c.ch.name, source_id: c.admin.source_id, state: "live", viewers, warm: viewers === 0, pinned: false,
      uptime_s: t - (warm.get(chId) ?? t), bitrate_kbps: 3200 + Math.round(Math.sin(t / 5 + chId) * 900), bytes_total: (t - (warm.get(chId) ?? t)) * 420_000,
      reconnects: chId % 4 === 0 ? 1 : 0, failovers: 0, active_candidate: 0, cc_errors: chId % 3, pts_discontinuities: 0, sync_losses: 0,
      keyframes: (t - (warm.get(chId) ?? t)) / 2 | 0, viewer_resyncs: 0, gop_bytes: 640_000, ttfb_ms: 180 + (chId % 7) * 40,
      time_to_keyframe_ms: 420 + (chId % 5) * 120, connected_s: t - (warm.get(chId) ?? t), video_codec: "h264", audio_codec: chId % 2 ? "aac" : "mp2",
      upstream_host: "line.harbour-iptv.example", last_error: null,
    };
  };

  const test = (b: { kind: string; url: string; username?: string | null }): SourceTest => {
    if (!/^https?:\/\//.test(b.url)) return { ok: false, message: "That address doesn't look like a URL (it should start with http:// or https://)." };
    if (/fail|bad/.test(b.url)) return { ok: false, message: "The provider rejected the username or password (HTTP 401)." };
    if (b.kind === "xtream")
      return {
        ok: true, message: "Connected — account active.", channels: 1284, epg_url: `${b.url.replace(/\/$/, "")}/xmltv.php`,
        account: { active: true, is_trial: false, status: "Active", exp_date: nowS() + 86400 * 143, max_connections: 2, active_connections: sessions.size ? 1 : 0 },
      };
    return { ok: true, message: "Playlist loaded.", channels: 312, epg_url: "https://example.org/epg/guide.xml.gz", account: null };
  };

  return {
    async channels(o = {}) {
      await latency();
      const q = (o.q ?? "").toLowerCase();
      return chans
        .filter((c) => c.admin.enabled)
        .filter((c) => (!o.group || c.ch.group === o.group) && (!o.favorites || c.ch.favorite) && (!q || c.ch.name.toLowerCase().indexOf(q) >= 0))
        .map((c) => ({ ...c.ch }));
    },
    async groups() {
      await latency();
      const m = new Map<string, number>();
      chans.forEach((c) => c.admin.enabled && m.set(c.ch.group ?? "Other", (m.get(c.ch.group ?? "Other") ?? 0) + 1));
      return Array.from(m, ([name, count]) => ({ name, count }));
    },
    async epg(q) {
      await latency(120, 380);
      if (q.to - q.from > 48 * 3600) throw new ApiError(400, "Window too large (max 48 h)");
      const out: EpgGrid = { from: q.from, to: q.to, channels: [] };
      q.channels.slice(0, 500).forEach((id) => {
        const c = byId.get(id);
        if (!c || !c.ch.has_epg) return out.channels.push({ channel_id: id, programs: [] });
        const programs = schedule(c).filter((p) => p.stop > q.from && p.start < q.to).map((p) => (q.detail ? { ...p } : compact(p)));
        out.channels.push({ channel_id: id, programs });
      });
      return out;
    },
    async now(ids) {
      await latency(60, 160);
      const t = nowS();
      const list = ids && ids.length ? ids : chans.slice(0, 500).map((c) => c.ch.id);
      return list.map((id): NowNext => {
        const c = byId.get(id);
        if (!c || !c.ch.has_epg) return { channel_id: id, now: null, next: null };
        const s = schedule(c);
        const i = s.findIndex((p) => p.start <= t && p.stop > t);
        return { channel_id: id, now: i >= 0 ? s[i]! : null, next: i >= 0 ? s[i + 1] ?? null : null };
      });
    },
    async favorite(id, on) {
      await latency(60, 140);
      need(id).ch.favorite = on;
    },
    async tune(id, body) {
      const c = need(id);
      // Warm channels zap faster — like the real hub's GOP cache.
      await latency(warm.has(id) ? 40 : 220, warm.has(id) ? 120 : 620);
      const takeover = !!(body as { takeover?: boolean }).takeover;
      // Big mode: a 1-connection plan, held by another user (admin → 409 + holders; takeover frees it).
      if (m === "big" && !bigFree && c.ch.id !== busyChannel) {
        if (!takeover)
          throw Object.assign(new ApiError(409, "All 1 connection(s) of this provider are in use by other users. Retry with takeover=true to stop their stream."), {
            body: {
              takeover: true,
              holders: [{ user_id: 7, username: "sam", session_id: "tv-1", channel_id: busyChannel, channel_name: byId.get(busyChannel)?.ch.name ?? "?", since: nowS() - 900 }],
            },
          });
        sessions.delete("tv-1");
        bigFree = true;
      }
      // Channel 13: both connections held by another user — the admin 409 with holders (new contract).
      if (c.ch.number === 13 && !takeover)
        throw Object.assign(new ApiError(409, "All 2 connection(s) of this provider are in use by other users. Retry with takeover=true to stop their stream."), {
          body: {
            takeover: true,
            holders: [
              { user_id: 7, username: "sam", session_id: "x-1", channel_id: 111, channel_name: "Arena One", since: nowS() - 600 },
              { user_id: 7, username: "sam", session_id: "x-2", channel_id: 121, channel_name: "Cine Classics", since: nowS() - 300 },
            ],
          },
        });
      if (c.ch.number === 27) throw new ApiError(503, "Channel did not deliver a stream");
      const id2 = `live-${(sid++).toString(36)}-${id}`;
      if (!warm.has(id)) warm.set(id, nowS());
      const mode = body.mode ?? "hls";
      sessions.set(id2, { id: id2, user_id: 1, username: "rafik", channel_id: id, channel_name: c.ch.name, mode, started_at: nowS(), idle_s: 0, connections: mode === "passthrough" ? 1 : 0, hls: mode === "hls" ? { video: "copy", audio: id % 2 ? "copy" : "aac", encoder: "copy" } : null });
      return {
        session_id: id2, mode, channel_id: id, channel_name: c.ch.name,
        // The dev server's test pattern stands in for the HLS stream.
        url: `${SAMPLE_VIDEO}?ch=${id}`,
        hls: mode === "hls" ? { video: "copy", audio: id % 2 ? "copy" : "aac", encoder: "copy" } : null,
      };
    },
    async stop(s) {
      await latency(30, 80);
      const x = sessions.get(s);
      if (!x) throw new ApiError(404, "Live session not found");
      sessions.delete(s);
      if (s === "tv-1") bigFree = true;
      // Previous channel stays warm for a grace period.
      if (x) setTimeout(() => !Array.from(sessions.values()).some((y) => y.channel_id === x.channel_id) && warm.delete(x.channel_id), 45_000);
    },

    // ---- admin ----------------------------------------------------------------
    async sources() {
      await latency();
      sources.forEach((s) => (s.open_connections = s.id === 1 ? Math.min(s.max_connections ?? 9, warm.size) : 0));
      return sources.map((s) => ({ ...s }));
    },
    async source(id) {
      await latency();
      const s = sources.find((x) => x.id === id);
      if (!s) throw new ApiError(404, "Source not found");
      return { ...s };
    },
    async createSource(b: SourceBody) {
      await latency(200, 400);
      if (!b.name.trim() || !b.url.trim()) throw new ApiError(400, "Name and URL are required");
      const s: SourceDto = {
        id: nextSource++, name: b.name, kind: b.kind, url: b.url, username: b.username ?? null, password_set: !!b.password, enabled: b.enabled ?? true,
        channel_count: 0, refreshing: true, open_connections: 0, max_connections: b.max_connections ?? null, last_refresh_at: null, last_epg_at: null,
        last_error: null, epg_url: b.epg_url ?? null, detected_epg_url: null, refresh_interval: b.refresh_interval ?? null, user_agent: b.user_agent ?? null,
      };
      sources.push(s);
      setTimeout(() => {
        s.refreshing = false;
        s.channel_count = b.kind === "xtream" ? 1284 : 312;
        s.last_refresh_at = nowS();
        s.last_epg_at = nowS();
      }, 4000);
      return { ...s };
    },
    async updateSource(id, b) {
      await latency();
      const s = sources.find((x) => x.id === id);
      if (!s) throw new ApiError(404, "Source not found");
      const { password, ...rest } = b;
      for (const k in rest) {
        const v = (rest as Record<string, unknown>)[k];
        if (v !== undefined) (s as unknown as Record<string, unknown>)[k] = v;
      }
      if (b.max_connections === 0) s.max_connections = null;
      if (b.epg_url === "") s.epg_url = null;
      if (password) s.password_set = true;
      return { ...s };
    },
    async deleteSource(id) {
      await latency();
      const i = sources.findIndex((x) => x.id === id);
      if (i < 0) throw new ApiError(404, "Source not found");
      sources.splice(i, 1);
    },
    async refreshSource(id) {
      await latency();
      const s = sources.find((x) => x.id === id);
      if (!s) throw new ApiError(404, "Source not found");
      if (s.refreshing) throw new ApiError(409, "A refresh is already running");
      s.refreshing = true;
      setTimeout(() => {
        s.refreshing = false;
        s.last_refresh_at = nowS();
        s.last_epg_at = nowS();
        s.last_error = null;
      }, 3500);
    },
    async testSource(id) {
      await latency(400, 900);
      const s = sources.find((x) => x.id === id);
      if (!s) throw new ApiError(404, "Source not found");
      return test(s);
    },
    async testConfig(b) {
      await latency(500, 1100);
      return test(b);
    },
    async adminChannels(o = {}) {
      await latency();
      const q = (o.q ?? "").toLowerCase();
      return chans
        .filter((c) => (o.source == null || c.admin.source_id === o.source) && (!o.group || c.admin.group === o.group) && (!q || c.admin.name.toLowerCase().indexOf(q) >= 0 || String(c.admin.number ?? "") === q))
        .filter((c) => o.hidden !== false || (c.admin.enabled && !c.admin.missing))
        .map((c) => ({ ...c.admin }));
    },
    async patchChannel(id, b) {
      await latency(80, 200);
      const c = need(id);
      if (b.enabled != null) c.admin.enabled = b.enabled;
      if (b.name != null) c.admin.name = c.ch.name = b.name;
      if ("number" in b) c.admin.number = c.ch.number = b.number ?? null;
      if (b.group != null) c.admin.group = c.ch.group = b.group;
      if ("epg_channel_id" in b) {
        c.admin.epg_channel_id = b.epg_channel_id ?? c.admin.tvg_id ?? null;
        c.admin.epg_manual = b.epg_channel_id != null;
        c.ch.has_epg = !!c.admin.epg_channel_id;
      }
      chans.sort((a, z) => (a.ch.number ?? 1e9) - (z.ch.number ?? 1e9) || a.ch.name.localeCompare(z.ch.name));
    },
    async setAliases(id, a) {
      await latency();
      need(id).admin.aliases = a;
    },
    async hub(): Promise<HubStatus> {
      await latency(60, 140);
      const t = nowS();
      sessions.forEach((s) => (s.idle_s = Math.min(8, t - s.started_at)));
      return {
        transcode: { encoder: "vaapi", fell_back_to_software: false },
        sources: sources.map((s) => ({ source_id: s.id, open_connections: s.id === 1 ? warm.size + (m === "big" && !bigFree ? 1 : 0) : 0, max_connections: s.max_connections ?? null })),
        channels: Array.from(warm.keys()).map(metrics),
        sessions: Array.from(sessions.values()),
      };
    },
    async liveSessions() {
      await latency();
      return Array.from(sessions.values());
    },
  };
}
