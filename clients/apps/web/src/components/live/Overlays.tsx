import { For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import type { IptvChannel, Program } from "../../api/iptv";
import type { ZapSample } from "./tuner";
import { ChannelLogo } from "./ChannelLogo";
import { clock, fmtRange, fmtTime, minutesLeft, nowNext, progressOf } from "./store";

/** Channel identity + current/next programme. `overlay` sits on the picture; `panel` under it. */
export function NowPlaying(props: { channel: IptvChannel; variant: "overlay" | "panel"; tuning?: boolean; children?: JSX.Element }): JSX.Element {
  const nn = () => nowNext(props.channel.id);
  const now = () => nn()?.now ?? null;
  const next = () => nn()?.next ?? null;
  return (
    <div class={`nowp nowp--${props.variant}`} classList={{ "is-tuning": !!props.tuning }}>
      <div class="nowp__id">
        <ChannelLogo channel={props.channel} class="chlogo--lg" />
        <div class="nowp__ch">
          <span class="nowp__num">{props.channel.number ?? ""}</span>
          <span class="nowp__name">
            {props.channel.name}
            <Show when={props.channel.favorite}><Icon name="star" class="nowp__fav" /></Show>
          </span>
          <Show when={props.channel.group}><span class="nowp__group">{props.channel.group}</span></Show>
        </div>
        <span class="nowp__live"><i /> Live</span>
      </div>
      <Show when={now()} fallback={<p class="nowp__none">{nn() ? "No guide information for this channel." : "\u00a0"}</p>}>
        {(p) => <Current program={p()} detail={props.variant === "panel"} />}
      </Show>
      <Show when={next()}>
        {(p) => (
          <p class="nowp__next">
            <span class="nowp__label">Next</span> <span class="nowp__time">{fmtTime(p().start)}</span> {p().title}
            <Show when={p().sub_title}><span class="nowp__sub"> · {p().sub_title}</span></Show>
          </p>
        )}
      </Show>
      {props.children}
    </div>
  );
}

function Current(props: { program: Program; detail: boolean }): JSX.Element {
  const p = () => props.program;
  return (
    <div class="nowp__cur">
      <h2 class="nowp__title">{p().title}</h2>
      <Show when={p().sub_title}><p class="nowp__subtitle">{p().sub_title}</p></Show>
      <div class="nowp__timeline">
        <span class="nowp__time">{fmtTime(p().start)}</span>
        <span class="nowp__bar" aria-hidden="true"><i style={{ width: `${progressOf(p(), clock()) * 100}%` }} /></span>
        <span class="nowp__time">{fmtTime(p().stop)}</span>
        <span class="nowp__left">{minutesLeft(p(), clock())}</span>
      </div>
      <Show when={props.detail && p().description}>
        <p class="nowp__desc">{p().description}</p>
      </Show>
    </div>
  );
}

/** "1 2 _" — digits typed on the remote, tunes after a short pause. */
export function NumberEntry(props: { digits: string; max: number; target?: IptvChannel; missing?: boolean }): JSX.Element {
  const slots = () => {
    const out: string[] = props.digits.split("");
    while (out.length < props.max) out.push("_");
    return out;
  };
  return (
    <div class="numentry" role="status" aria-live="assertive" classList={{ "is-missing": !!props.missing }}>
      <div class="numentry__digits">
        <For each={slots()}>{(d) => <span classList={{ "is-empty": d === "_" }}>{d}</span>}</For>
      </div>
      <Show when={props.target} fallback={<span class="numentry__name">{props.missing ? "No such channel" : "\u00a0"}</span>}>
        {(c) => (
          <span class="numentry__name">
            <ChannelLogo channel={c()} /> {c().name}
          </span>
        )}
      </Show>
    </div>
  );
}

/** Debug overlay: time-to-first-frame for recent zaps. */
export function ZapStats(props: { samples: ZapSample[]; mode?: string; hls?: { video: string; audio: string; encoder: string } | null; onClose: () => void }): JSX.Element {
  const avg = (k: "apiMs" | "ttffMs", warm?: boolean) => {
    const l = props.samples.filter((s) => warm == null || s.warm === warm);
    return l.length ? Math.round(l.reduce((a, s) => a + s[k], 0) / l.length) : null;
  };
  const ms = (v: number | null) => (v == null ? "—" : v >= 1000 ? `${(v / 1000).toFixed(2)} s` : `${v} ms`);
  return (
    <div class="zapstats" role="dialog" aria-label="Zap timing">
      <header class="zapstats__head">
        <strong><Icon name="activity" /> Zap timing</strong>
        <button type="button" class="iconbtn zapstats__x" aria-label="Close timing" onClick={props.onClose}><Icon name="x" /></button>
      </header>
      <dl class="zapstats__sum">
        <div><dt>Last</dt><dd class="is-hero">{ms(props.samples[0]?.ttffMs ?? null)}</dd></div>
        <div><dt>Cold avg</dt><dd>{ms(avg("ttffMs", false))}</dd></div>
        <div><dt>Warm avg</dt><dd>{ms(avg("ttffMs", true))}</dd></div>
        <div><dt>API avg</dt><dd>{ms(avg("apiMs"))}</dd></div>
      </dl>
      <Show when={props.mode}>
        <p class="zapstats__mode">
          {props.mode === "passthrough" ? "Passthrough (MPEG-TS)" : "HLS"}
          {props.hls ? ` · video ${props.hls.video}${props.hls.encoder !== "copy" ? ` (${props.hls.encoder})` : ""} · audio ${props.hls.audio}` : ""}
        </p>
      </Show>
      <ol class="zapstats__list">
        <For each={props.samples} fallback={<li class="zapstats__empty">Change channel to measure.</li>}>
          {(s) => (
            <li>
              <span class="zapstats__ch">{s.channel}</span>
              <span class="zapstats__tag" classList={{ "is-warm": s.warm }}>{s.warm ? "warm" : "cold"}</span>
              <span class="zapstats__v">{ms(s.apiMs)}</span>
              <span class="zapstats__v is-strong">{ms(s.ttffMs)}</span>
            </li>
          )}
        </For>
      </ol>
      <p class="zapstats__hint">Tune API · first frame (from tune). Press Blue or I to hide.</p>
    </div>
  );
}

export { fmtRange };
