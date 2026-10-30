// Placeholder icon generator (no dependencies): dark tile + accent ring + play triangle, written as PNG.
import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

const crcTable = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf: Uint8Array): number {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type: string, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + data.length);
  const dv = new DataView(out.buffer);
  dv.setUint32(0, data.length);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  out.set(data, 8);
  dv.setUint32(8 + data.length, crc32(out.subarray(4, 8 + data.length)));
  return out;
}

const BG = [14, 15, 19], ACCENT = [255, 138, 61];
const mix = (a: number[], b: number[], t: number) => a.map((v, i) => v + (b[i]! - v) * t);

/** Render pixel (x,y) in a unit square centred on the mark (coordinates in px relative to the mark size `s`). */
function shade(x: number, y: number, s: number): number[] {
  const cx = x - s / 2, cy = y - s / 2;
  const r = Math.hypot(cx, cy);
  const ringO = s * 0.36, ringI = s * 0.30;
  const aa = Math.max(1, s / 400);
  let col = BG;
  const ring = Math.min(1, Math.max(0, Math.min(ringO - r, r - ringI) / aa + 0.5));
  col = mix(col, ACCENT, ring);
  // play triangle
  const tw = s * 0.2, th = s * 0.24;
  const tx = cx + tw * 0.35; // optical centring
  if (tx > -tw / 2 && tx < tw / 2) {
    const frac = (tx + tw / 2) / tw; // 0..1 left→right
    const half = (th / 2) * (1 - frac);
    const d = half - Math.abs(cy);
    col = mix(col, ACCENT, Math.min(1, Math.max(0, d / aa + 0.5)));
  }
  return col;
}

export function writeIcon(path: string, w: number, h: number): void {
  const s = Math.min(w, h) * (w === h ? 1 : 0.5); // splash: mark is half the height
  const ox = (w - s) / 2, oy = (h - s) / 2;
  const raw = new Uint8Array((w * 3 + 1) * h);
  let p = 0;
  for (let y = 0; y < h; y++) {
    raw[p++] = 0;
    for (let x = 0; x < w; x++) {
      const inside = x >= ox && x < ox + s && y >= oy && y < oy + s;
      const c = inside ? shade(x - ox, y - oy, s) : BG;
      raw[p++] = c[0]!; raw[p++] = c[1]!; raw[p++] = c[2]!;
    }
  }
  const ihdr = new Uint8Array(13);
  const dv = new DataView(ihdr.buffer);
  dv.setUint32(0, w); dv.setUint32(4, h);
  ihdr[8] = 8; ihdr[9] = 2; // 8-bit RGB
  const png = Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", new Uint8Array(0)),
  ]);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, png);
}
