/**
 * Procedural SVG artwork for mock mode: posters, backdrops, stills and headshots.
 * Offline, deterministic, and deliberately "designed" so screens are reviewable
 * without real TMDB art.
 */

export function rng(seed: number): () => number {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return ((s >>> 0) % 100000) / 100000;
  };
}

export function hash(str: string): number {
  let h = 2166136261;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** [deep, mid, light, accent] */
const PALETTES: [string, string, string, string][] = [
  ["#0d1b2a", "#1b3a5c", "#e0c9a6", "#ff7849"],
  ["#1a0f1f", "#4a1d3f", "#f2d0a9", "#ff4d6d"],
  ["#081c15", "#1b4332", "#d8f3dc", "#f4a261"],
  ["#14110f", "#3d2c22", "#f1e3c8", "#e9c46a"],
  ["#0b132b", "#3a506b", "#e7ecef", "#5bc0be"],
  ["#20090b", "#6a040f", "#ffd6a5", "#ffba08"],
  ["#10002b", "#3c096c", "#e0aaff", "#ff9e00"],
  ["#0f1a12", "#2d3a1f", "#ece4d4", "#9fd356"],
  ["#1c1c1c", "#4a4e69", "#f2e9e4", "#c9ada7"],
  ["#03071e", "#370617", "#faa307", "#e85d04"],
  ["#0a1128", "#034078", "#fefcfb", "#1282a2"],
  ["#1b1b1e", "#373f51", "#d8dbe2", "#a9bcd0"],
  ["#2b0f0e", "#7f2b0a", "#f8e9d2", "#3fa7a1"],
  ["#071a1a", "#0f4c5c", "#f2f2e9", "#fb8b24"],
];

function palette(seed: number) {
  return PALETTES[seed % PALETTES.length]!;
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function uri(svg: string): string {
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}

function wrap(title: string, max: number): string[] {
  const words = title.toUpperCase().split(/\s+/);
  const lines: string[] = [];
  let cur = "";
  for (const w of words) {
    if ((cur + " " + w).trim().length > max && cur) {
      lines.push(cur);
      cur = w;
    } else cur = (cur + " " + w).trim();
  }
  if (cur) lines.push(cur);
  return lines.slice(0, 4);
}

const memo = new Map<string, string>();
function cached(key: string, fn: () => string): string {
  let v = memo.get(key);
  if (!v) memo.set(key, (v = fn()));
  return v;
}

export function posterArt(title: string, seed: number, tagline = ""): string {
  return cached(`p:${seed}:${title}`, () => {
    const r = rng(seed);
    const [deep, mid, light, acc] = palette(seed);
    const kind = seed % 5;
    let art = "";
    if (kind === 0) {
      // Sun over striped sea
      const cy = 95 + r() * 30;
      art = `<circle cx="100" cy="${cy}" r="${48 + r() * 18}" fill="${acc}"/>`;
      for (let i = 0; i < 9; i++)
        art += `<rect x="0" y="${cy + 6 + i * 9}" width="200" height="${2 + i * 0.7}" fill="${deep}"/>`;
      art += `<rect x="0" y="${cy + 60}" width="200" height="200" fill="${deep}"/>`;
    } else if (kind === 1) {
      // Monolith with frame
      const w = 34 + r() * 30;
      art = `<rect x="${100 - w / 2}" y="40" width="${w}" height="170" fill="url(#g)"/>`;
      art += `<rect x="14" y="14" width="172" height="272" fill="none" stroke="${light}" stroke-opacity=".35" stroke-width="1.2"/>`;
      art += `<circle cx="${100 + (r() - 0.5) * 50}" cy="${60 + r() * 30}" r="5" fill="${acc}"/>`;
    } else if (kind === 2) {
      // Contours
      const cx = 60 + r() * 80, cy = 90 + r() * 50;
      for (let i = 9; i > 0; i--)
        art += `<circle cx="${cx}" cy="${cy}" r="${i * 15}" fill="none" stroke="${i % 3 === 0 ? acc : light}" stroke-opacity="${0.15 + (i % 3 === 0 ? 0.6 : 0.1)}" stroke-width="${i % 3 === 0 ? 2.2 : 1}"/>`;
    } else if (kind === 3) {
      // Halftone field
      for (let y = 0; y < 13; y++)
        for (let x = 0; x < 10; x++) {
          const d = Math.hypot(x - 6.5, y - 4.5);
          const rad = Math.max(0.4, 8.6 - d * 1.05);
          art += `<circle cx="${x * 20 + 10}" cy="${y * 20 + 10}" r="${rad.toFixed(1)}" fill="${y > 7 ? mid : acc}" fill-opacity="${y > 7 ? 0.9 : 0.85}"/>`;
        }
    } else {
      // Diagonal split
      const k = 120 + r() * 80;
      art = `<polygon points="0,${k} 200,${k - 110} 200,300 0,300" fill="${mid}"/>`;
      art += `<polygon points="0,${k + 16} 200,${k - 94} 200,${k - 88} 0,${k + 22}" fill="${acc}"/>`;
      art += `<circle cx="${40 + r() * 120}" cy="${50 + r() * 30}" r="${10 + r() * 16}" fill="${light}" fill-opacity=".9"/>`;
    }
    const lines = wrap(title, kind === 3 ? 10 : 12);
    const fs = lines.length > 2 ? 19 : 23;
    const serif = seed % 3 === 0;
    const font = serif ? "Georgia,'Times New Roman',serif" : "'Helvetica Neue',Arial,sans-serif";
    const baseY = 286 - (lines.length - 1) * fs * 0.98 - (tagline ? 10 : 0);
    const text = lines
      .map(
        (l, i) =>
          `<text x="16" y="${(baseY + i * fs * 0.98).toFixed(1)}" font-family="${font}" font-weight="${serif ? 700 : 900}" font-size="${fs}" letter-spacing="${serif ? 0 : -0.6}" fill="${light}">${esc(l)}</text>`,
      )
      .join("");
    const tag = tagline
      ? `<text x="16" y="291" font-family="'Helvetica Neue',Arial,sans-serif" font-size="7" letter-spacing="1.6" fill="${light}" fill-opacity=".7">${esc(tagline.toUpperCase())}</text>`
      : "";
    return uri(
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 300" preserveAspectRatio="xMidYMid slice">` +
        `<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${acc}"/><stop offset="1" stop-color="${mid}"/></linearGradient>` +
        `<linearGradient id="v" x1="0" y1="0" x2="0" y2="1"><stop offset=".45" stop-color="${deep}" stop-opacity="0"/><stop offset="1" stop-color="${deep}" stop-opacity=".92"/></linearGradient></defs>` +
        `<rect width="200" height="300" fill="${deep}"/>${art}<rect width="200" height="300" fill="url(#v)"/>${text}${tag}</svg>`,
    );
  });
}

export function backdropArt(seed: number): string {
  return cached(`b:${seed}`, () => {
    const r = rng(seed * 7 + 3);
    const [deep, mid, light, acc] = palette(seed);
    const sx = 60 + r() * 220, sy = 50 + r() * 50, sr = 14 + r() * 26;
    const ridge = (base: number, amp: number, n: number) => {
      let d = `M0,180 L0,${base}`;
      for (let i = 0; i <= n; i++) d += ` L${(i * 320) / n},${(base - r() * amp).toFixed(1)}`;
      return d + " L320,180 Z";
    };
    return uri(
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 180" preserveAspectRatio="xMidYMid slice">` +
        `<defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${deep}"/><stop offset=".62" stop-color="${mid}"/><stop offset="1" stop-color="${acc}" stop-opacity=".8"/></linearGradient>` +
        `<radialGradient id="h" cx="${sx / 320}" cy="${sy / 180}" r=".55"><stop offset="0" stop-color="${acc}" stop-opacity=".75"/><stop offset="1" stop-color="${acc}" stop-opacity="0"/></radialGradient></defs>` +
        `<rect width="320" height="180" fill="url(#s)"/><rect width="320" height="180" fill="url(#h)"/>` +
        `<circle cx="${sx}" cy="${sy}" r="${sr}" fill="${light}" fill-opacity=".92"/>` +
        `<path d="${ridge(118, 34, 7)}" fill="${mid}" fill-opacity=".85"/>` +
        `<path d="${ridge(140, 26, 11)}" fill="${deep}" fill-opacity=".9"/>` +
        `<path d="${ridge(162, 14, 16)}" fill="${deep}"/></svg>`,
    );
  });
}

export function headshotArt(name: string): string {
  const seed = hash(name);
  return cached(`h:${seed}`, () => {
    const [deep, mid, light, acc] = palette(seed);
    const initials = name
      .split(/\s+/)
      .map((w) => w.charAt(0))
      .join("")
      .slice(0, 2)
      .toUpperCase();
    return uri(
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">` +
        `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${mid}"/><stop offset="1" stop-color="${deep}"/></linearGradient></defs>` +
        `<rect width="100" height="100" fill="url(#g)"/>` +
        `<circle cx="50" cy="42" r="17" fill="${acc}" fill-opacity=".28"/><path d="M18,100 C22,72 78,72 82,100Z" fill="${acc}" fill-opacity=".28"/>` +
        `<text x="50" y="58" text-anchor="middle" font-family="'Helvetica Neue',Arial,sans-serif" font-weight="800" font-size="30" fill="${light}">${esc(initials)}</text></svg>`,
    );
  });
}
