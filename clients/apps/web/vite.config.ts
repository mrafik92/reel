import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";
import { createReadStream, existsSync, mkdirSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
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

export default defineConfig({
  plugins: [solid(), mockMedia()],
  base: "./", // relative paths so the bundle works inside webOS/Tizen packages
  server: {
    proxy: { "/api": { target: "http://localhost:8420", changeOrigin: true, ws: true } },
  },
  // hls.js is lazy-loaded only for HLS sources, so its size doesn't hit first paint.
  build: { target: "es2019", cssTarget: "chrome68", chunkSizeWarningLimit: 700 },
});
