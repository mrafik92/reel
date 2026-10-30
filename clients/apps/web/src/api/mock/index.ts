import type { Api } from "../index";
import { ApiError } from "../index";
import type { ItemSummary, LibrarySort } from "@reel/ui";
import { buildDb, detail, home } from "./fixtures";

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const latency = () => sleep(120 + Math.random() * 220);

function sortItems(items: ItemSummary[], sort: LibrarySort): ItemSummary[] {
  const key = (s: string) => s.replace(/^(the|a|an)\s+/i, "").toLowerCase();
  const copy = items.slice();
  if (sort === "title") copy.sort((a, b) => (key(a.title) < key(b.title) ? -1 : key(a.title) > key(b.title) ? 1 : 0));
  else if (sort === "added") copy.sort((a, b) => (a.added_at < b.added_at ? 1 : -1));
  else copy.sort((a, b) => (b.year ?? 0) - (a.year ?? 0) || (key(a.title) < key(b.title) ? -1 : 1));
  return copy;
}

function fold(s: string): string {
  return s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");
}

export function createMockApi(): Api {
  const db = buildDb();
  if (typeof console !== "undefined") console.info("[reel] mock mode — fixture data, no server calls");
  const need = <T>(v: T | undefined, what: string): T => {
    if (v === undefined) throw new ApiError(404, `${what} not found`);
    return v;
  };

  return {
    mock: true,
    async libraries() {
      await latency();
      return db.libraries.map((l) => ({ ...l }));
    },
    async libraryItems(id, { sort, cursor, limit = 60 }) {
      await latency();
      const ids = need(db.byLibrary.get(Number(id)), "Library");
      const all = sortItems(ids.map((i) => db.byId.get(i)!.summary), sort);
      const start = cursor ? parseInt(atob(cursor), 10) || 0 : 0;
      const items = all.slice(start, start + limit).map((s) => ({ ...s }));
      const next = start + limit < all.length ? btoa(String(start + limit)) : null;
      return { items, next_cursor: next };
    },
    async item(id) {
      await latency();
      return need(detail(db, Number(id)), "Item");
    },
    async children(id) {
      await latency();
      const rec = need(db.byId.get(Number(id)), "Item");
      return rec.childIds.map((c) => ({ ...db.byId.get(c)!.summary }));
    },
    async home() {
      await latency();
      return home(db);
    },
    async search(q, limit = 60) {
      await sleep(80 + Math.random() * 120);
      const needle = fold(q.trim());
      if (!needle) return [];
      const out: ItemSummary[] = [];
      db.byId.forEach((r) => {
        if ((r.summary.kind === "movie" || r.summary.kind === "show") && fold(r.summary.title).indexOf(needle) >= 0) {
          out.push(r.summary);
        }
      });
      // Prefix matches first.
      out.sort((a, b) => Number(fold(b.title).indexOf(needle) === 0) - Number(fold(a.title).indexOf(needle) === 0));
      return out.slice(0, limit);
    },
    async reportProgress(r) {
      const rec = db.byId.get(Number(r.item_id));
      if (!rec || !r.duration_s) return;
      const p = r.position_s / r.duration_s;
      if (p > 0.95 || r.reason === "ended") {
        rec.summary.played = true;
        rec.summary.progress = null;
      } else if (p > 0.01) {
        rec.summary.progress = p;
      }
      console.debug("[reel] progress (mock)", r);
    },
  };
}
