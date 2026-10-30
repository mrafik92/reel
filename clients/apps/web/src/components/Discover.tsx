import { createSignal, Show, type JSX } from "solid-js";
import { DiscoverCard as Card, Icon } from "@reel/ui";
import type { DiscoverCard, MediaType } from "../api";
import { progressFor } from "../lib/downloads";
import { canAutoAdd } from "../lib/auth";
import { isBusy, useRequestActions } from "../lib/requests";
import { DiscoverSheet } from "./DiscoverSheet";
import { vodTwin } from "./vod/links";

/** One sheet per page; tiles call `open(card)`. */
export function useDiscoverSheet() {
  const [open, setOpen] = createSignal<DiscoverCard>();
  const view = (): JSX.Element => (
    <Show when={open()}>
      {(c) => <DiscoverSheet media={c().media_type as MediaType} tmdbId={c().tmdb_id} seed={c()} onClose={() => setOpen(undefined)} />}
    </Show>
  );
  return { open: (c: DiscoverCard) => setOpen(c), view };
}

/** A TMDB card wired to the global progress store and request actions. */
export function DiscoverTile(props: { card: DiscoverCard; onOpen: (c: DiscoverCard) => void; autofocus?: boolean; subtitle?: string }): JSX.Element {
  const actions = useRequestActions();
  // Reel request progress, else raw Radarr/Sonarr queue activity for this title.
  const dl = () => progressFor(props.card.tmdb_id);
  const twin = () => vodTwin(props.card);
  const card = () => (
    <Card
      card={props.card}
      download={dl()}
      addLabel={canAutoAdd() ? "Add" : "Request"}
      busy={isBusy(props.card.media_type, props.card.tmdb_id)}
      autofocus={props.autofocus}
      subtitle={props.subtitle}
      onOpen={() => props.onOpen(props.card)}
      onAdd={() =>
        // Partly-available series open the sheet to pick missing seasons; everything else is one click.
        props.card.media_type === "tv" && props.card.status === "partially" ? props.onOpen(props.card) : void actions.add(props.card)
      }
      onPlay={() => void actions.play(props.card)}
      onRetry={dl()?.request_id ? () => void actions.retry(dl()!.request_id, props.card.title) : undefined}
    />
  );
  return (
    <Show when={twin()} fallback={card()}>
      {/* Not in the library but on IPTV: Play now sits right under Add. */}
      <div class="dtile">
        {card()}
        <a class="pill pill--iptv" href={twin()!} aria-label={`Play now on IPTV: ${props.card.title}`}>
          <Icon name="live" /> <span>Play now</span> <small>IPTV</small>
        </a>
      </div>
    </Show>
  );
}
