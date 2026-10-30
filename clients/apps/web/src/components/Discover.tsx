import { createSignal, Show, type JSX } from "solid-js";
import { DiscoverCard as Card } from "@reel/ui";
import type { DiscoverCard, MediaType } from "../api";
import { downloads } from "../lib/downloads";
import { canAutoAdd } from "../lib/auth";
import { isBusy, useRequestActions } from "../lib/requests";
import { DiscoverSheet } from "./DiscoverSheet";

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
export function DiscoverTile(props: { card: DiscoverCard; onOpen: (c: DiscoverCard) => void; autofocus?: boolean }): JSX.Element {
  const actions = useRequestActions();
  const dl = () => downloads.get(props.card.tmdb_id);
  return (
    <Card
      card={props.card}
      download={dl()}
      addLabel={canAutoAdd() ? "Add" : "Request"}
      busy={isBusy(props.card.media_type, props.card.tmdb_id)}
      autofocus={props.autofocus}
      onOpen={() => props.onOpen(props.card)}
      onAdd={() =>
        // Partly-available series open the sheet to pick missing seasons; everything else is one click.
        props.card.media_type === "tv" && props.card.status === "partially" ? props.onOpen(props.card) : void actions.add(props.card)
      }
      onPlay={() => void actions.play(props.card)}
      onRetry={dl() ? () => void actions.retry(dl()!.request_id, props.card.title) : undefined}
    />
  );
}
