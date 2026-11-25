import { createResource, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { vod, type VodId, type VodKind } from "../../api/vod";
import { CategoryLabel } from "./Categories";
import { TitleSheet } from "../TitleSheet";
import "../../styles/vod.css";

/** "IPTV · Film · AR | Egyptian Movies" — where this title comes from. */
export function VodEyebrow(props: { kind: VodKind; categoryId: VodId }): JSX.Element {
  const [cats] = createResource(() => props.kind, (k) => vod.categories(k).catch(() => []));
  const cat = () => (cats() ?? []).find((c) => String(c.id) === String(props.categoryId));
  return (
    <p class="eyebrow veyebrow">
      <span class="vbadge vbadge--inline"><Icon name="live" /> IPTV</span>
      <span>{props.kind === "movie" ? "Film" : "Series"}</span>
      <Show when={cat()}>
        <span class="veyebrow__cat"><CategoryLabel name={cat()!.name} /></span>
      </Show>
    </p>
  );
}

/** Small print under the actions: what IPTV playback means. */
export function VodFineprint(props: { local?: boolean }): JSX.Element {
  return (
    <p class="vfine">
      <Icon name="live" />
      <span>
        {props.local
          ? "Your library copy plays from your own server. The IPTV copy streams from your provider, one screen at a time."
          : "Streams from your IPTV provider — one screen at a time across your household."}
      </span>
    </p>
  );
}

export function VodSkeleton(): JSX.Element {
  return (
    <TitleSheet head={
      <div aria-busy="true" aria-label="Loading">
        <span class="skel skel--title" />
        <span class="skel skel--text" style={{ width: "14rem", height: "3.25rem" }} />
      </div>
    }>
      <section class="tinfo">
        <div class="tinfo__main">
          <span class="skel skel--text" style={{ width: "60%" }} />
          <span class="skel skel--text" style={{ width: "90%" }} />
          <span class="skel skel--text" style={{ width: "80%" }} />
        </div>
      </section>
    </TitleSheet>
  );
}
