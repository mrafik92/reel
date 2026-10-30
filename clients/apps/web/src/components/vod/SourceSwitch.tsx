import { createResource, Show, type JSX } from "solid-js";
import { Icon, type LibraryKind } from "@reel/ui";
import { vod } from "../../api/vod";

export type Source = "library" | "iptv";

const KEY = (k: LibraryKind) => `reel.source.${k}`;

export function storedSource(kind: LibraryKind): Source | undefined {
  try {
    const v = localStorage.getItem(KEY(kind));
    return v === "iptv" || v === "library" ? v : undefined;
  } catch {
    return undefined;
  }
}
export function rememberSource(kind: LibraryKind, s: Source): void {
  try {
    localStorage.setItem(KEY(kind), s);
  } catch {
    /* storage disabled */
  }
}

/** Does the server have an IPTV on-demand catalogue for this kind? (No provider / old server → no switch.) */
export function useVodCount(kind: LibraryKind) {
  const [n] = createResource(
    () => kind,
    (k) => vod.categories(k === "movies" ? "movie" : "series").then((cs) => cs.reduce((a, c) => a + c.count, 0), () => 0),
  );
  return n;
}

/** My library · IPTV — which shelf Movies / Shows shows. */
export function SourceSwitch(props: { value: Source; libraryCount?: number; iptvCount?: number; onChange: (s: Source) => void }): JSX.Element {
  const fmt = (n?: number) => (n == null ? "" : n >= 10000 ? `${Math.round(n / 1000)}k` : n.toLocaleString());
  return (
    <div class="srcswitch" role="group" aria-label="Source">
      <button
        type="button"
        class="srcswitch__btn"
        aria-pressed={props.value === "library"}
        data-nav-default={props.value === "library" ? "" : undefined}
        onClick={() => props.onChange("library")}
      >
        <Icon name="server" class="srcswitch__icon" />
        <span>My library</span>
        <Show when={props.libraryCount != null}><span class="srcswitch__n">{fmt(props.libraryCount)}</span></Show>
      </button>
      <button
        type="button"
        class="srcswitch__btn srcswitch__btn--iptv"
        aria-pressed={props.value === "iptv"}
        data-nav-default={props.value === "iptv" ? "" : undefined}
        onClick={() => props.onChange("iptv")}
      >
        <Icon name="live" class="srcswitch__icon" />
        <span>IPTV</span>
        <Show when={props.iptvCount != null}><span class="srcswitch__n">{fmt(props.iptvCount)}</span></Show>
      </button>
    </div>
  );
}
