import { Show, type JSX } from "solid-js";
import { Icon, Img } from "@reel/ui";
import type { VodCard as Card } from "../../api/vod";
import { vodHref } from "./links";
import "../../styles/vod.css";

/**
 * An IPTV title, dressed like a library poster: same frame, focus and caption, plus a small
 * source badge — "IPTV", or "In library" when the same title is on the server (local wins).
 */
export function VodCard(props: {
  card: Card;
  /** "IPTV" source badge — on where titles from different sources mix (search); off inside the IPTV browser. */
  badge?: boolean;
  subtitle?: string;
  autofocus?: boolean;
  onFocus?: (c: Card) => void;
}): JSX.Element {
  const c = () => props.card;
  const progress = () => {
    const p = c().progress;
    return p && p > 0.005 && p < 0.995 ? p : 0;
  };
  const rating = () => (c().rating && c().rating! > 0 ? c().rating!.toFixed(1) : "");
  // "Series" only matters where films and series mix (search); the Shows page already says so.
  const sub = () => props.subtitle ?? [c().year ?? "", c().kind === "series" && props.badge !== false ? "Series" : ""].filter(Boolean).join(" · ");
  const inLib = () => !!c().library_item_id;
  return (
    <a
      class="card card--poster vcard"
      classList={{ "vcard--lib": inLib() }}
      href={vodHref(c())}
      data-fid={`v:${c().kind}:${c().id}`}
      data-autofocus={props.autofocus ? "" : undefined}
      onFocus={() => props.onFocus?.(c())}
      aria-label={`${c().title}${sub() ? ", " + sub() : ""}${rating() ? ", rated " + rating() : ""}${inLib() ? ", also in your library" : ", on IPTV"}`}
    >
      <div class="card__frame">
        <Img
          class="card__img"
          src={c().poster_url}
          width={342}
          alt=""
          fallback={
            <div class="card__fallback vcard__fallback" aria-hidden="true">
              <Icon name="live" class="vcard__fallback-icon" />
              <span dir="auto">{c().title}</span>
            </div>
          }
        />
        <Show when={inLib()} fallback={
          <Show when={props.badge !== false}>
            <span class="vbadge" aria-hidden="true"><Icon name="live" /> IPTV</span>
          </Show>
        }>
          <span class="avail avail--available vbadge--lib" aria-hidden="true"><Icon name="check" /> In library</span>
        </Show>
        <Show when={c().played}>
          <span class="card__played" title="Watched"><Icon name="check" /></span>
        </Show>
        <Show when={rating()}>
          <span class="card__meta" classList={{ "has-progress": !!progress() }} aria-hidden="true">
            <span class="card__rating"><Icon name="star" filled /> {rating()}</span>
          </span>
        </Show>
        <Show when={progress()}>
          <div class="card__progress" aria-hidden="true"><i style={{ width: `${progress() * 100}%` }} /></div>
        </Show>
      </div>
      <div class="card__caption" dir="auto">
        <span class="card__title">{c().title}</span>
        <Show when={sub()}><span class="card__sub">{sub()}</span></Show>
      </div>
    </a>
  );
}
