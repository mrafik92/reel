// Generates every Reel icon from the one source SVG (#142), so the sizes can't drift:
//   web favicon (SVG + ICO), apple-touch-icon, PWA manifest icons (any + maskable),
//   webOS icon / largeIcon / splash and the Tizen package icon.
// Run from clients/: `bun run icons`. The outputs are committed; CI doesn't run this.
import { Resvg } from "@resvg/resvg-js";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

/** Tile colour of logo.svg (`--accent` in ui.css): painted behind the full-bleed and maskable icons. */
const TILE = "#c2352d";
/** App background (`--bg` in ui.css): the webOS splash screen. */
const BG = "#0b0909";

const clients = resolve(import.meta.dir, "../../..");
const logo = readFileSync(resolve(import.meta.dir, "../assets/logo.svg"), "utf8").trim();

/** The logo placed at (x, y, size) inside a w×h canvas, optionally on a solid background. */
function compose(w: number, h: number, size: number, bg?: string): string {
  const x = (w - size) / 2, y = (h - size) / 2;
  const placed = logo.replace("<svg ", `<svg x="${x}" y="${y}" width="${size}" height="${size}" `);
  const fill = bg ? `<rect width="${w}" height="${h}" fill="${bg}"/>` : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${fill}${placed}</svg>`;
}

function png(svg: string, w: number): Buffer {
  return Buffer.from(new Resvg(svg, { fitTo: { mode: "width", value: w } }).render().asPng());
}

/** Rounded tile on transparency: favicons, launcher icons. */
const tile = (s: number) => png(compose(512, 512, 512), s);
/** Full-bleed square (no transparent corners); `scale` < 1 keeps the mark in the maskable safe zone. */
const bleed = (s: number, scale = 1) => png(compose(512, 512, 512 * scale, TILE), s);

/** ICO with PNG-encoded entries (supported by every current browser). */
function ico(sizes: number[]): Buffer {
  const imgs = sizes.map(tile);
  const head = Buffer.alloc(6 + 16 * imgs.length);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(imgs.length, 4);
  let offset = head.length;
  imgs.forEach((img, i) => {
    const e = 6 + 16 * i, s = sizes[i]!;
    head.writeUInt8(s >= 256 ? 0 : s, e);
    head.writeUInt8(s >= 256 ? 0 : s, e + 1);
    head.writeUInt16LE(1, e + 4); // colour planes
    head.writeUInt16LE(32, e + 6); // bits per pixel
    head.writeUInt32LE(img.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += img.length;
  });
  return Buffer.concat([head, ...imgs]);
}

const out: Record<string, Buffer | string> = {
  "apps/web/public/favicon.svg": logo.replace(/\s*<!--[\s\S]*?-->/g, "") + "\n",
  "apps/web/public/favicon.ico": ico([16, 32, 48]),
  "apps/web/public/apple-touch-icon.png": bleed(180), // iOS rounds the corners itself
  "apps/web/public/icon-192.png": tile(192),
  "apps/web/public/icon-512.png": tile(512),
  "apps/web/public/icon-maskable-192.png": bleed(192, 0.8),
  "apps/web/public/icon-maskable-512.png": bleed(512, 0.8),
  "apps/webos/assets/icon.png": tile(80),
  "apps/webos/assets/largeIcon.png": tile(130),
  "apps/webos/assets/splash.png": png(compose(1920, 1080, 360, BG), 1920),
  "apps/tizen/assets/icon.png": tile(512),
};

for (const [rel, data] of Object.entries(out)) {
  const path = resolve(clients, rel);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, data);
  console.log("wrote", rel);
}
