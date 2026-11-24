import { createEffect, createResource, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { useLocation } from "@solidjs/router";
import { formatRuntime, Icon, Img, onBack, type ItemDetail } from "@reel/ui";
import { api, invalidateLibrary } from "../api";
import { continueSubtitle } from "../lib/continue";
import { itemHref, playHref } from "../lib/links";
import { closePreview, keepPreview, previewScrolled, previewTarget, type PreviewTarget } from "../lib/preview";
import { toast } from "../lib/toast";

const LEAVE_MS = 120;

/** The single floating preview. It lives above the page because rows clip their content. */
export function PreviewLayer(): JSX.Element {
  const loc = useLocation();
  // Keep the card mounted through its 120 ms exit.
  const [shown, setShown] = createSignal<PreviewTarget | null>(null);
  const [leaving, setLeaving] = createSignal(false);
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;

  createEffect(() => {
    const t = previewTarget();
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
      offBack();
      closePreview();
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
      toast(e instanceof Error ? e.message : "Couldn't update", "error");
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
  const playLink = () => (it.kind === "show" ? itemHref(it) : playHref(it));
  const infoLink = () => itemHref(it.series ? { id: it.series.id, kind: "show" } : it);

  onMount(() => {
    const frame = t.el.querySelector(".card__frame") ?? t.el;
    const r = frame.getBoundingClientRect();
    const root = document.documentElement;
    const rem = parseFloat(getComputedStyle(root).fontSize) || 16;
    const gutter = parseFloat(getComputedStyle(root).getPropertyValue("--gutter")) * rem || 48;
    const bar = (parseFloat(getComputedStyle(root).getPropertyValue("--topbar-h")) || 0) * rem;
    const vw = root.clientWidth;
    const vh = window.innerHeight;
    const width = Math.min(Math.max(r.width * 1.9, 20 * rem), vw - 2 * gutter);
    el.style.width = `${width}px`;
    const height = el.offsetHeight;
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    const left = Math.max(gutter, Math.min(cx - width / 2, vw - gutter - width));
    const top = Math.max(bar + 8, Math.min(cy - height / 2, vh - height - 8));
    setStyle({
      width: `${width}px`,
      left: `${left}px`,
      top: `${top}px`,
      "transform-origin": `${cx - left}px ${cy - top}px`,
    });
    const raf = requestAnimationFrame(() => requestAnimationFrame(() => setEntered(true)));
    // The card carries the focus ring now: the tile's own would show around its edge.
    if (!hover) t.el.classList.add("is-previewed");
    onCleanup(() => {
      cancelAnimationFrame(raf);
      t.el.classList.remove("is-previewed");
    });
  });

  return (
    <div
      ref={el}
      class="preview glass"
      classList={{ "preview--focus": !hover, "is-in": entered() && !props.leaving }}
      style={style()}
      aria-hidden="true"
      data-nav-skip
      onMouseEnter={keepPreview}
      onMouseLeave={() => hover && closePreview()}
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
        <Show when={it.series}>
          <p class="preview__episode">{continueSubtitle(it)}</p>
        </Show>
        <p class="preview__meta">
          <Show when={rating()}>
            <span class="preview__rating"><Icon name="star" filled /> {rating()}</span>
          </Show>
          <Show when={it.year}><span>{detail()?.year ?? it.year}</span></Show>
          <Show when={detail()?.content_rating}><span class="badge">{detail()!.content_rating}</span></Show>
          <Show when={length()}><span>{length()}</span></Show>
        </p>
        <Show when={genres().length}>
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
        </Show>
      </div>
    </div>
  );
}
