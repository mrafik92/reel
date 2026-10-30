export interface VttCue { start: number; end: number; text: string }

function ts(s: string): number {
  const p = s.trim().replace(",", ".").split(":").map(Number);
  while (p.length < 3) p.unshift(0);
  return (p[0] ?? 0) * 3600 + (p[1] ?? 0) * 60 + (p[2] ?? 0);
}

/** Tiny WebVTT/SRT parser (tags stripped) — used where the platform has no native <track>. */
export function parseVtt(src: string): VttCue[] {
  const cues: VttCue[] = [];
  for (const block of src.replace(/\r/g, "").split(/\n{2,}/)) {
    const lines = block.split("\n");
    const i = lines.findIndex((l) => l.includes("-->"));
    if (i < 0) continue;
    const m = /([\d:.,]+)\s*-->\s*([\d:.,]+)/.exec(lines[i]!);
    if (!m) continue;
    const text = lines.slice(i + 1).join("\n").replace(/<[^>]+>/g, "").trim();
    if (text) cues.push({ start: ts(m[1]!), end: ts(m[2]!), text });
  }
  return cues;
}
