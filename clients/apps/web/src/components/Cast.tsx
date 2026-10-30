import { Show, type JSX } from "solid-js";
import { Img } from "@reel/ui";
import { href } from "../lib/mode";

/** Cast member: links to their person page when TMDB knows them. */
export function CastTile(props: { c: { name: string; character?: string | null; photo_url?: string | null; person_tmdb_id?: number | null } }): JSX.Element {
  const inner = () => (
    <>
      <Img
        class="person__photo"
        src={props.c.photo_url}
        width={185}
        fallback={<span class="person__initials">{props.c.name.split(" ").map((w) => w.charAt(0)).join("").slice(0, 2)}</span>}
      />
      <span class="person__name">{props.c.name}</span>
      <Show when={props.c.character}><span class="person__role">{props.c.character}</span></Show>
    </>
  );
  return (
    <Show
      when={props.c.person_tmdb_id}
      fallback={<div class="person" tabindex="0" data-focusable data-fid={"p:" + props.c.name}>{inner()}</div>}
    >
      <a class="person person--link" href={href(`/person/${props.c.person_tmdb_id}`)} data-fid={"p:" + props.c.name} aria-label={`${props.c.name}${props.c.character ? ` as ${props.c.character}` : ""} — see filmography`}>
        {inner()}
      </a>
    </Show>
  );
}
