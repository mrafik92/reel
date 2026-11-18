import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";
import { createReadStream, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/**
 * Dev-only: serves `/mock-media/sample.webm` for mock mode (`?mock=1`).
 * Generated on first request with ffmpeg (test pattern + tone, VP9/Opus so it
 * plays in plain Chromium); never part of the production bundle.
 */
function mockMedia(): Plugin {
  const file = resolve(here, "mock-media/sample.webm");
  const ensure = () => {
    if (existsSync(file)) return true;
    mkdirSync(dirname(file), { recursive: true });
    const r = spawnSync("ffmpeg", [
      "-hide_banner", "-loglevel", "error", "-y",
      "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=24:duration=90",
      "-f", "lavfi", "-i", "sine=frequency=220:duration=90",
      "-c:v", "libvpx-vp9", "-b:v", "300k", "-deadline", "realtime", "-cpu-used", "8",
      "-c:a", "libopus", "-b:a", "48k", "-shortest", file,
    ]);
    return r.status === 0 && existsSync(file);
  };
  return {
    name: "reel-mock-media",
    apply: "serve",
    configureServer(server) {
      // Multi-variant HLS (#111) for mock adaptive sessions: `master.m3u8` lists 480p / 720p /
      // original, `master-transcode.m3u8` 480p / 720p / 1080p (a transcode-only title; 1080p
      // serves the original's files). Each variant has its own test pattern, so a switch shows.
      server.middlewares.use("/mock-media/abr/", (req, res) => {
        const dir = resolve(here, "mock-media/abr");
        if (!ensureAbr(dir)) {
          res.statusCode = 404;
          res.end("mock media unavailable (ffmpeg not found)");
          return;
        }
        const path = (req.url ?? "").split("?")[0]!.replace(/^\/+/, "").replace(/^1080p\//, "original/");
        const file = resolve(dir, path);
        if (!file.startsWith(dir) || !existsSync(file) || !statSync(file).isFile()) {
          res.statusCode = 404;
          res.end("not found");
          return;
        }
        const type: Record<string, string> = { ".m3u8": "application/vnd.apple.mpegurl", ".m4s": "video/iso.segment", ".mp4": "video/mp4" };
        res.setHeader("Content-Type", type[extname(file)] ?? "application/octet-stream");
        res.setHeader("Cache-Control", "no-store");
        createReadStream(file).pipe(res);
      });
      server.middlewares.use("/mock-media/subs.vtt", (req, res) => {
        const lang = /lang=([a-z]+)/.exec(req.url ?? "")?.[1] ?? "eng";
        const line = lang === "fre" ? "Les lumières du port s'allument une à une." : lang === "spa" ? "Las luces del puerto se encienden." : "The harbour lights come on, one by one.";
        let body = "WEBVTT\n\n";
        for (let t = 0; t < 90; t += 6) {
          const f = (s: number) => `00:00:${String(s).padStart(2, "0")}.000`;
          body += `${f(t)} --> ${f(Math.min(89, t + 4))}\n${line} (${t}s)\n\n`;
        }
        res.setHeader("Content-Type", "text/vtt");
        res.end(body);
      });
      server.middlewares.use("/mock-media/sample.webm", (req, res) => {
        if (!ensure()) {
          res.statusCode = 404;
          res.end("mock media unavailable (ffmpeg not found)");
          return;
        }
        const size = statSync(file).size;
        const range = /bytes=(\d*)-(\d*)/.exec(req.headers.range ?? "");
        res.setHeader("Content-Type", "video/webm");
        res.setHeader("Accept-Ranges", "bytes");
        if (range) {
          const start = range[1] ? parseInt(range[1], 10) : 0;
          const end = range[2] ? Math.min(parseInt(range[2], 10), size - 1) : size - 1;
          res.statusCode = 206;
          res.setHeader("Content-Range", `bytes ${start}-${end}/${size}`);
          res.setHeader("Content-Length", String(end - start + 1));
          createReadStream(file, { start, end }).pipe(res);
        } else {
          res.setHeader("Content-Length", String(size));
          createReadStream(file).pipe(res);
        }
      });
    },
  };
}

/** Mock variants: id, test pattern, size, video kbit/s. */
const ABR_VARIANTS = [
  { id: "480p", src: "smptehdbars", w: 854, h: 480, kbps: 600 },
  { id: "720p", src: "testsrc", w: 1280, h: 720, kbps: 1200 },
  { id: "original", src: "testsrc2", w: 1920, h: 1080, kbps: 2500 },
];

/** Generate the mock multi-variant HLS once (90 s, 4 s segments, H.264/AAC fMP4). */
function ensureAbr(dir: string): boolean {
  if (existsSync(resolve(dir, "master-transcode.m3u8"))) return true;
  for (const v of ABR_VARIANTS) {
    const out = resolve(dir, v.id);
    mkdirSync(out, { recursive: true });
    const r = spawnSync("ffmpeg", [
      "-hide_banner", "-loglevel", "error", "-y",
      "-f", "lavfi", "-i", `${v.src}=size=${v.w}x${v.h}:rate=24:duration=90`,
      "-f", "lavfi", "-i", "sine=frequency=330:duration=90",
      "-c:v", "libx264", "-preset", "veryfast", "-profile:v", "main", "-pix_fmt", "yuv420p",
      "-b:v", `${v.kbps}k`, "-maxrate", `${Math.round(v.kbps * 1.2)}k`, "-bufsize", `${v.kbps * 2}k`,
      "-force_key_frames", "expr:gte(t,n_forced*4)", "-sc_threshold", "0",
      "-c:a", "aac", "-b:a", "96k", "-ac", "2", "-shortest",
      "-f", "hls", "-hls_time", "4", "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
      "-hls_fmp4_init_filename", "init.mp4", "-hls_segment_filename", resolve(out, "seg_%05d.m4s"),
      resolve(out, "index.m3u8"),
    ]);
    if (r.status !== 0) return false;
  }
  const inf = (v: (typeof ABR_VARIANTS)[number], id: string) =>
    `#EXT-X-STREAM-INF:BANDWIDTH=${(Math.round(v.kbps * 1.2) + 96) * 1000},AVERAGE-BANDWIDTH=${(v.kbps + 96) * 1000},` +
    `RESOLUTION=${v.w}x${v.h},CODECS="avc1.4d4028,mp4a.40.2"\n${id}/index.m3u8\n`;
  const head = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n";
  writeFileSync(resolve(dir, "master.m3u8"), head + ABR_VARIANTS.map((v) => inf(v, v.id)).join(""));
  writeFileSync(resolve(dir, "master-transcode.m3u8"), head + ABR_VARIANTS.map((v) => inf(v, v.id === "original" ? "1080p" : v.id)).join(""));
  return readFileSync(resolve(dir, "master.m3u8"), "utf8").length > 0;
}

export default defineConfig({
  plugins: [solid(), mockMedia()],
  // Web (served by reel-server, deep links like /item/42) needs absolute asset paths; the
  // webOS/Tizen packages load from file:// and need relative ones (package.sh sets VITE_TARGET=tv).
  base: process.env.VITE_TARGET === "tv" ? "./" : "/",
  server: {
    // REEL_PROXY_TARGET=https://reel.example.com bun run dev → develop against a remote server.
    proxy: { "/api": { target: process.env.REEL_PROXY_TARGET || "http://localhost:8420", changeOrigin: true, ws: true, secure: true } },
  },
  // hls.js is lazy-loaded only for HLS sources, so its size doesn't hit first paint.
  build: { target: "es2019", cssTarget: "chrome68", chunkSizeWarningLimit: 700 },
});
