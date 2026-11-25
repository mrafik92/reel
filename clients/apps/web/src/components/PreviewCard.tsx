import { createEffect, createResource, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { useLocation } from "@solidjs/router";
import { formatRuntime, Icon, Img, onBack, type ItemDetail } from "@reel/ui";
import { api, invalidateLibrary } from "../api";
import { continueSubtitle } from "../lib/continue";
import { itemHref, playHref } from "../lib/links";
import { closePreview, keepPreview, leavePreview, previewScrolled, previewTarget, type PreviewTarget } from "../lib/preview";
import { previewPlacement, previewWidth, qualityBadge, translateXOf, type PreviewKind } from "../lib/previewGeometry";
import { toast } from "../lib/toast";

/** The card's exit (CSS `.preview` transition). */
const LEAVE_MS = 130;

// ---------------------------------------------------------------------------
// The row makes room (#301). The open tile fades out under the card and the tiles before and after
// it slide aside with transforms, never layout: the row's scroll position, scroll width as the
// paging arrows measure it, and the TV navigation's idea of where a tile is all stay put.

let openTile: HTMLElement | null = null;
let parted: HTMLElement[] = [];

/** Let the row close up again and give the tile back. */
function closeRow(): void {
  for (const el of parted) el.style.transform = "";
  parted = [];
  openTile?.classList.remove("is-previewed");
  openTile = null;
}

function partRow(tile: HTMLElement, before: number, after: number): void {
  closeRow();
  openTile = tile;
  tile.classList.add("is-previewed");
  const track = tile.parentElement;
  if (!track || !track.classList.contains("row__track")) return;
  const shift = (el: HTMLElement, dx: number) => {
    if (Math.abs(dx) < 0.5) return;
    el.style.transform = `translateX(${dx.toFixed(1)}px)`;
    parted.push(el);
  };
  let seen = false;
  for (let i = 0; i < track.children.length; i++) {
    const el = track.children[i] as HTMLElement;
    if (el === tile) {
      seen = true;
      // The numbered row: the open tile's numeral goes with the tiles before it.
      const rank = tile.querySelector<HTMLElement>(".card__rank");
      if (rank) shift(rank, before);
      continue;
    }
    shift(el, seen ? after : before);
  }
}

function kindOf(tile: HTMLElement): PreviewKind {
  if (tile.classList.contains("card--ranked")) return "ranked";
  return tile.classList.contains("card--landscape") ? "landscape" : "poster";
}

/** The single floating preview. It lives above the page because rows clip their content. */
export function PreviewLayer(): JSX.Element {
  const loc = useLocation();
  // Keep the card mounted through its 120 ms exit.
  const [shown, setShown] = createSignal<PreviewTarget | null>(null);
  const [leaving, setLeaving] = createSignal(false);
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;

  createEffect(() => {
    const t = previewTarget();
    // The row starts closing with the card, not after its exit.
    closeRow();
    if (leaveTimer) clearTimeout(leaveTimer);
    if (t) {
      setLeaving(false);
      setShown(t);
    } else if (shown()) {
      setLeaving(true);
      leaveTimer = setTimeout(() => {
        setShown(null);
        setLeaving(false);
      }, LEAVE_MS);
    }
  });
  createEffect(on(() => loc.pathname, closePreview, { defer: true }));

  onMount(() => {
    const onScroll = () => previewScrolled();
    const onDown = (e: Event) => {
      const t = e.target as Element | null;
      if (!t || !t.closest || !t.closest(".preview")) closePreview();
    };
    // Focus previews follow focus by themselves; a key press dismisses a mouse preview.
    const onKey = () => {
      if (previewTarget()?.mode === "hover") closePreview();
    };
    // Capture: scrolling inside a row doesn't bubble to window.
    window.addEventListener("scroll", onScroll, { capture: true, passive: true });
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    // The card and its parted row were placed for the old layout.
    window.addEventListener("resize", closePreview);
    const offBack = onBack(() => {
      const t = previewTarget();
      if (!t) return false;
      closePreview();
      // A focus card opens by itself on every tile, so it must not cost the viewer a Back press.
      return t.mode === "hover";
    });
    onCleanup(() => {
      if (leaveTimer) clearTimeout(leaveTimer);
      window.removeEventListener("scroll", onScroll, { capture: true } as EventListenerOptions);
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", closePreview);
      offBack();
      closePreview();
      closeRow();
    });
  });

  return (
    <Show when={shown()} keyed>
      {(t) => (
        <Portal>
          <Card target={t} leaving={leaving()} />
        </Portal>
      )}
    </Show>
  );
}

function Card(props: { target: PreviewTarget; leaving: boolean }): JSX.Element {
  const t = props.target;
  const it = t.item;
  const hover = t.mode === "hover";
  let el!: HTMLDivElement;
  const [style, setStyle] = createSignal<JSX.CSSProperties>({ visibility: "hidden" });
  const [entered, setEntered] = createSignal(false);

  // An episode in Continue watching describes its series; `it` already wears the series' title and art.
  const detailId = it.series?.id ?? it.id;
  const [detail] = createResource(detailId, (id) => api.item(id).catch(() => undefined as ItemDetail | undefined));
  const [played, setPlayed] = createSignal(it.played);
  const [saving, setSaving] = createSignal(false);

  const toggle = async () => {
    if (saving()) return;
    const next = !played();
    setSaving(true);
    setPlayed(next);
    try {
      await api.setPlayed(it.id, next);
      invalidateLibrary();
    } catch (e) {
      setPlayed(!next);
      toast(e instanceof Error ? e.message : "Couldn’t update", "error");
    } finally {
      setSaving(false);
    }
  };

  const progress = () => (it.progress && it.progress > 0.005 && it.progress < 0.995 && !played() ? it.progress : 0);
  const rating = () => {
    const r = detail()?.rating ?? it.rating;
    return r && r > 0 ? r.toFixed(1) : "";
  };
  const length = () => {
    const d = detail();
    const kind = d?.kind ?? it.kind;
    if (kind === "show") {
      const n = d?.children_count;
      return n ? `${n} ${n === 1 ? "season" : "seasons"}` : "";
    }
    return it.series ? "" : formatRuntime(d?.runtime_s ?? it.runtime_s);
  };
  const genres = () => (detail()?.genres ?? it.genres ?? []).slice(0, 3);
  const quality = () => qualityBadge(detail()?.files);
  // A title in progress shows where the viewer is instead of the synopsis. Fixed when the card opens
  // (from the tile, not the detail or Mark watched), so the card's height never changes while open.
  const p0 = it.progress ?? 0;
  const resumeLine = it.series || (p0 > 0.005 && p0 < 0.995 && !it.played) ? continueSubtitle(it) : "";
  const resume = () => resumeLine;
  const playLink = () => (it.kind === "show" ? itemHref(it) : playHref(it));
  const infoLink = () => itemHref(it.series ? { id: it.series.id, kind: "show" } : it);

  onMount(() => {
    const tile = t.el;
    const root = document.documentElement;
    const css = getComputedStyle(root);
    const rem = parseFloat(css.fontSize) || 16;
    const gutter = parseFloat(css.getPropertyValue("--gutter")) * rem || 48;
    const bar = (parseFloat(css.getPropertyValue("--topbar-h")) || 0) * rem;
    const vw = root.clientWidth;
    const vh = window.innerHeight;
    // Where the art sits in the layout: without the slide of a row that is still closing up, and
    // without the TV focus scale (offset sizes ignore transforms).
    const r = tile.getBoundingClientRect();
    const x = r.left - translateXOf(getComputedStyle(tile).transform);
    const frame = tile.querySelector<HTMLElement>(".card__frame");
    const art = frame
      ? { left: x + frame.offsetLeft, top: r.top + frame.offsetTop, width: frame.offsetWidth, height: frame.offsetHeight }
      : { left: x, top: r.top, width: r.width, height: r.height };
    const kind = kindOf(tile);
    const width = previewWidth(kind, art.width, rem, vw, gutter);
    el.style.width = `${width}px`;
    const head = tile.closest(".row")?.querySelector(".row__head");
    const p = previewPlacement({
      kind,
      tile: art,
      width,
      height: el.offsetHeight,
      headBottom: head ? head.getBoundingClientRect().bottom : null,
      viewportWidth: vw,
      viewportHeight: vh,
      gutter,
      minTop: bar + 0.5 * rem,
      gap: 0.25 * rem,
    });
    setStyle({
      width: `${width}px`,
      left: `${p.left}px`,
      top: `${p.top}px`,
      "transform-origin": `${p.originX}px ${p.originY}px`,
    });
    partRow(tile, p.before, p.after);
    const raf = requestAnimationFrame(() => requestAnimationFrame(() => setEntered(true)));
    onCleanup(() => cancelAnimationFrame(raf));
  });

  return (
    <div
      ref={el}
      class="preview"
      classList={{ "preview--focus": !hover, "is-in": entered() && !props.leaving }}
      style={style()}
      aria-hidden="true"
      data-nav-skip
      onMouseEnter={keepPreview}
      onMouseLeave={() => hover && leavePreview()}
    >
      <a class="preview__art" href={t.href} tabindex="-1">
        <Img
          src={it.backdrop_url ?? it.poster_url}
          width={780}
          eager
          class={it.backdrop_url ? undefined : "preview__img--poster"}
          fallback={<div class="preview__noart" />}
        />
        <span class="preview__shade" />
        <span class="preview__title">{it.title}</span>
        <Show when={quality()}>
          <span class="preview__quality">{quality()}</span>
        </Show>
        <Show when={progress()}>
          <span class="preview__progress"><i style={{ width: `${progress() * 100}%` }} /></span>
        </Show>
      </a>
      <div class="preview__body">
        <Show when={hover}>
          <div class="preview__actions">
            <a class="iconbtn iconbtn--solid" href={playLink()} tabindex="-1" title="Play" aria-label="Play">
              <Icon name="play" />
            </a>
            <button
              type="button"
              class="iconbtn iconbtn--ring"
              tabindex="-1"
              title={played() ? "Mark unwatched" : "Mark watched"}
              aria-label={played() ? "Mark unwatched" : "Mark watched"}
              aria-pressed={played()}
              onClick={() => void toggle()}
            >
              <Icon name={played() ? "eyeoff" : "check"} />
            </button>
            <a class="iconbtn iconbtn--ring preview__more" href={infoLink()} tabindex="-1" title="More info" aria-label="More info">
              <Icon name="down" />
            </a>
          </div>
        </Show>
        <Show when={resume()}>
          <p class="preview__episode">{resume()}</p>
        </Show>
        <p class="preview__meta">
          <Show when={rating()}>
            <span class="preview__rating"><Icon name="star" filled /> {rating()}</span>
          </Show>
          <Show when={detail()?.year ?? it.year}>{(y) => <span>{y()}</span>}</Show>
          <Show when={detail()?.content_rating}><span class="badge">{detail()!.content_rating}</span></Show>
          <Show when={length()}><span>{length()}</span></Show>
        </p>
        <Show when={!resume()}>
          <p class="preview__synopsis">{detail()?.overview ?? ""}</p>
        </Show>
        <p class="preview__genres">
          <For each={genres()}>
            {(g, i) => (
              <>
                <Show when={i() > 0}><i aria-hidden="true">•</i></Show>
                <span>{g}</span>
              </>
            )}
          </For>
        </p>
      </div>
    </div>
  );
}
