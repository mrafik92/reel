import { createMemo, createSignal, For, onCleanup, Show, type JSX } from "solid-js";
import { Icon, PICKER_LANGUAGES, Sheet, iso1, languageName } from "@reel/ui";
import { DEFAULT_PREFS, type PlaybackPrefs, type SubtitleMode } from "../api/prefs";
import { loadPrefs, playbackPrefs, prefsSupported, savePrefs } from "../lib/prefs";
import { previewSelection } from "../lib/tracks";
import { chooseLiveDelivery, directAllowed, liveDelivery, type LiveDelivery } from "../lib/liveDelivery";
import { chooseDeviceQuality, deviceQuality, fmtMbps, userLimitKbps } from "../lib/quality";
import type { Quality } from "../api";
import { Panel, Toggle } from "../components/AdminKit";
import "../styles/prefs.css";

/**
 * Settings → Playback (per user, every device): which audio and subtitles Reel picks when
 * you press Play. Changes save as you make them; a live example shows what they mean.
 * Rendered in the settings pane (`pages/admin/Admin.tsx`), next to the section nav (#88).
 */

const MODES: { id: SubtitleMode; title: string; body: string }[] = [
  { id: "off", title: "Off", body: "Subtitles never come on by themselves." },
  {
    id: "forced_only",
    title: "Only forced (signs & foreign parts)",
    body: "Just the bits in another language — signs, letters, a few foreign lines.",
  },
  {
    id: "auto",
    title: "Automatic — when the audio isn't in a language I know",
    body: "On for a film in a language that isn't in your lists below; off otherwise.",
  },
  { id: "always", title: "Always", body: "Always on, in the first of your subtitle languages a title has." },
];

const DELIVERY: { id: LiveDelivery; title: string; body: string }[] = [
  {
    id: "server",
    title: "Through Reel",
    body: "The default. Channels are shared and zapping is fast, and your provider login never leaves the server.",
  },
  {
    id: "direct",
    title: "Straight from the provider",
    body: "This device connects to the provider itself, so Reel carries no video. It still uses one of the provider's connections, and zapping is slower. The provider login is sent to this device.",
  },
];

const QUALITY_CHOICES: { id: Quality; title: string; body: string }[] = [
  {
    id: "auto",
    title: "Auto",
    body: "The default. Plays the original whenever it can, and converts only when this device can't play it or it's over your account's limit.",
  },
  {
    id: "original",
    title: "Original — never re-encode the picture",
    body: "Direct play or repackaging only (the sound may still be converted). If a title can't play that way here, Reel says why and offers to convert it.",
  },
  { id: "1080p", title: "1080p", body: "At most 1080p, about 8 Mbps. Smaller originals play as they are; nothing is upscaled." },
  { id: "720p", title: "720p", body: "At most 720p, about 4 Mbps. Good for a slower connection away from home." },
  { id: "480p", title: "480p", body: "At most 480p, about 2 Mbps. For a weak or metered connection." },
];

const langLabel = (c: string) => (c === "original" ? "Original language" : languageName(c));

export default function SettingsPlayback(): JSX.Element {
  // Focus stays with the settings layout (the nav's current link), like the other sections.
  void loadPrefs(true);

  const p = (): PlaybackPrefs => playbackPrefs() ?? DEFAULT_PREFS;
  const [status, setStatus] = createSignal<"idle" | "saving" | "saved" | "error">("idle");
  const [errText, setErrText] = createSignal("");
  const [adding, setAdding] = createSignal<"audio" | "sub" | null>(null);
  const off = () => prefsSupported() === false;

  // Edits show at once (draft) and save shortly after the last one, so arrow-key reordering
  // doesn't PUT on every press. The server's normalised copy replaces the draft.
  const [draft, setDraft] = createSignal<PlaybackPrefs>();
  const cur = (): PlaybackPrefs => draft() ?? p();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let fade: ReturnType<typeof setTimeout> | undefined;
  const flush = async () => {
    const v = draft();
    if (!v) return;
    try {
      await savePrefs(v);
      if (draft() === v) setDraft(undefined);
      setStatus("saved");
      fade = setTimeout(() => setStatus("idle"), 2400);
    } catch (e) {
      setDraft(undefined);
      setErrText(e instanceof Error ? e.message : "Couldn't save");
      setStatus("error");
    }
  };
  onCleanup(() => {
    if (fade) clearTimeout(fade);
    if (timer) {
      clearTimeout(timer);
      void flush();
    }
  });
  const change = (patch: Partial<PlaybackPrefs>) => {
    if (off()) return;
    setDraft({ ...cur(), ...patch });
    setStatus("saving");
    if (timer) clearTimeout(timer);
    if (fade) clearTimeout(fade);
    timer = setTimeout(() => {
      timer = undefined;
      void flush();
    }, 450);
  };

  const subListActive = () => cur().subtitle_mode === "auto" || cur().subtitle_mode === "always";

  return (
    <div class="prefs">
      <header class="prefs__head">
        <div class="prefs__headrow">
          <h2 class="prefs__title">Playback</h2>
          <p class="prefs__status" role="status" aria-live="polite" classList={{ [`is-${status()}`]: true }}>
            <Show when={status() === "saving"}>Saving…</Show>
            <Show when={status() === "saved"}><Icon name="check" /> Saved</Show>
            <Show when={status() === "error"}><Icon name="alert" /> {errText()}</Show>
          </p>
        </div>
        <p class="prefs__lede">
          How Reel picks quality, audio and subtitles when you press Play. Languages and subtitles are saved to your
          account, so they follow you to every device; quality is set per device. You can still change them while
          watching.
        </p>
      </header>

      <Show when={off()}>
        <p class="note prefs__note">
          <Icon name="alert" /> Your Reel server doesn't support playback preferences yet. Update the server to save
          these.
        </p>
      </Show>

      <div class="prefs__layout" classList={{ "is-off": off() }}>
        <div class="prefs__main">
          <Panel title="Audio language" icon="audio">
            <p class="panel__lede">We play the first language in this list that a title has.</p>
            <LangList
              value={cur().audio_languages}
              onChange={(v) => change({ audio_languages: v.length ? v : ["original"] })}
              minItems={1}
              disabled={off()}
            />
            <button type="button" class="btn btn--sm prefs__add" disabled={off()} onClick={() => setAdding("audio")}>
              <Icon name="plus" /> <span>Add a language</span>
            </button>
          </Panel>

          <Panel title="Quality on this device" icon="quality">
            <p class="panel__lede">
              Films and episodes start at this quality. Saved on this device only, since the right choice depends on its
              connection. You can still change it while watching.
              <Show when={userLimitKbps()}>{(l) => <> Your account is limited to {fmtMbps(l())}.</>}</Show>
            </p>
            <div class="prefs__modes" role="radiogroup" aria-label="Playback quality on this device" data-nav-group>
              <For each={QUALITY_CHOICES}>
                {(m) => (
                  <button
                    type="button"
                    role="radio"
                    class="prefs__mode"
                    aria-checked={deviceQuality() === m.id ? "true" : "false"}
                    onClick={() => chooseDeviceQuality(m.id)}
                  >
                    <span class="prefs__radio" aria-hidden="true"><i /></span>
                    <span class="prefs__modetext">
                      <strong>{m.title}</strong>
                      <small>{m.body}</small>
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Panel>

          <Show when={directAllowed()}>
            <Panel title="Live TV on this device" icon="tv">
              <p class="panel__lede">
                Where Live TV streams come from. Saved on this device only. If a channel can't play directly (browsers
                often can't), Reel switches that channel to the server.
              </p>
              <div class="prefs__modes" role="radiogroup" aria-label="Where Live TV streams come from" data-nav-group>
                <For each={DELIVERY}>
                  {(m) => (
                    <button
                      type="button"
                      role="radio"
                      class="prefs__mode"
                      aria-checked={liveDelivery() === m.id ? "true" : "false"}
                      onClick={() => chooseLiveDelivery(m.id)}
                    >
                      <span class="prefs__radio" aria-hidden="true"><i /></span>
                      <span class="prefs__modetext">
                        <strong>{m.title}</strong>
                        <small>{m.body}</small>
                      </span>
                    </button>
                  )}
                </For>
              </div>
            </Panel>
          </Show>

          <Panel title="Subtitles" icon="cc">
            <div class="prefs__modes" role="radiogroup" aria-label="When to show subtitles" data-nav-group>
              <For each={MODES}>
                {(m) => (
                  <button
                    type="button"
                    role="radio"
                    class="prefs__mode"
                    aria-checked={cur().subtitle_mode === m.id ? "true" : "false"}
                    disabled={off()}
                    onClick={() => change({ subtitle_mode: m.id })}
                  >
                    <span class="prefs__radio" aria-hidden="true"><i /></span>
                    <span class="prefs__modetext">
                      <strong>{m.title}</strong>
                      <small>{m.body}</small>
                    </span>
                  </button>
                )}
              </For>
            </div>

            <h3 class="prefs__sub" classList={{ "is-dim": !subListActive() }}>Subtitle languages</h3>
            <p class="panel__lede" classList={{ "is-dim": !subListActive() }}>
              In order of preference.{" "}
              {subListActive() ? "" : "Used with Automatic and Always."}
            </p>
            <LangList
              value={cur().subtitle_languages}
              onChange={(v) => change({ subtitle_languages: v })}
              minItems={0}
              disabled={off()}
              empty="No subtitle languages yet — add one so subtitles can come on."
            />
            <button type="button" class="btn btn--sm prefs__add" disabled={off()} onClick={() => setAdding("sub")}>
              <Icon name="plus" /> <span>Add a language</span>
            </button>

            <div class="prefs__toggles">
              <Toggle
                label="Prefer subtitles for the deaf and hard of hearing (SDH)"
                hint="Include sound cues like [door slams] and who's speaking, when a title has them."
                checked={cur().prefer_sdh}
                disabled={off()}
                onChange={(v) => change({ prefer_sdh: v })}
              />
              <Toggle
                label="Allow picture-based subtitles"
                hint="Some releases only have subtitles as images (PGS, VobSub). Showing them means the server re-encodes the video — more work, and a moment longer to start."
                checked={cur().burn_in_image_subs}
                disabled={off()}
                onChange={(v) => change({ burn_in_image_subs: v })}
              />
            </div>
          </Panel>
        </div>

        <aside class="prefs__side" aria-label="What this means">
          <Examples prefs={cur()} />
        </aside>
      </div>

      <Show when={adding()} keyed>
        {(kind) => (
          // `kind` is a plain value here (keyed): the sheet calls onSelect after it has closed.
          <Sheet
            title={kind === "audio" ? "Add an audio language" : "Add a subtitle language"}
            selected={""}
            options={addOptions(kind, kind === "audio" ? cur().audio_languages : cur().subtitle_languages)}
            onSelect={(v) =>
              kind === "audio"
                ? change({ audio_languages: cur().audio_languages.concat(v) })
                : change({ subtitle_languages: cur().subtitle_languages.concat(v) })
            }
            onClose={() => setAdding(null)}
          />
        )}
      </Show>
    </div>
  );
}

function addOptions(kind: "audio" | "sub", list: string[]): { value: string; title: string; detail?: string }[] {
  const have = list.map((c) => (c === "original" ? c : iso1(c)));
  const langs = PICKER_LANGUAGES.filter((c) => have.indexOf(c) < 0)
    .map((c) => ({ value: c, title: languageName(c) }))
    .sort((a, b) => a.title.localeCompare(b.title));
  return kind === "audio" && have.indexOf("original") < 0
    ? [{ value: "original", title: "Original language", detail: "Whatever the title was made in" }, ...langs]
    : langs;
}

/** Ordered list with move up/down and remove — arrow-key friendly (one row per line). */
function LangList(props: {
  value: string[];
  onChange: (v: string[]) => void;
  minItems: number;
  disabled?: boolean;
  empty?: string;
}): JSX.Element {
  const move = (i: number, d: number) => {
    const v = props.value.slice();
    const j = i + d;
    if (j < 0 || j >= v.length) return;
    const t = v[i]!;
    v[i] = v[j]!;
    v[j] = t;
    props.onChange(v);
    // Keep focus on the same button of the moved row.
    requestAnimationFrame(() => {
      const row = document.querySelector<HTMLElement>(`[data-lang-row="${t}"] [data-move="${d}"]`);
      if (row && !(row as HTMLButtonElement).disabled) row.focus();
      else document.querySelector<HTMLElement>(`[data-lang-row="${t}"] button:not([disabled])`)?.focus();
    });
  };
  return (
    <ol class="langlist">
      <For each={props.value} fallback={<li class="langlist__empty">{props.empty}</li>}>
        {(code, i) => (
          <li class="langlist__row" data-lang-row={code} data-nav-group>
            <span class="langlist__n">{i() + 1}</span>
            <span class="langlist__name">
              {langLabel(code)}
              <Show when={code === "original"}><small>The language the title was made in</small></Show>
            </span>
            <button type="button" class="iconbtn iconbtn--sm" data-move="-1" aria-label={`Move ${langLabel(code)} up`} disabled={props.disabled || i() === 0} onClick={() => move(i(), -1)}>
              <Icon name="up" />
            </button>
            <button type="button" class="iconbtn iconbtn--sm" data-move="1" aria-label={`Move ${langLabel(code)} down`} disabled={props.disabled || i() === props.value.length - 1} onClick={() => move(i(), 1)}>
              <Icon name="down" />
            </button>
            <button
              type="button"
              class="iconbtn iconbtn--sm"
              aria-label={`Remove ${langLabel(code)}`}
              disabled={props.disabled || props.value.length <= props.minItems}
              onClick={() => props.onChange(props.value.filter((c) => c !== code))}
            >
              <Icon name="x" />
            </button>
          </li>
        )}
      </For>
    </ol>
  );
}

/** Two sample titles run through the same rules as the server: "what will happen?" */
function Examples(props: { prefs: PlaybackPrefs }): JSX.Element {
  const first = () => props.prefs.audio_languages.map((l) => (l === "original" ? "" : iso1(l))).find(Boolean) || "en";
  // A film in a language that's (probably) not yours: Japanese, or French if you speak Japanese.
  const foreign = () => {
    const known = props.prefs.audio_languages.concat(props.prefs.subtitle_languages).map(iso1);
    return ["ja", "fr", "ko", "de"].find((l) => known.indexOf(l) < 0) ?? "ja";
  };
  const samples = createMemo(() => {
    const mine = first();
    const sub = (index: number, language: string, o: { forced?: boolean; title?: string; codec?: string } = {}) => ({
      index, language, forced: !!o.forced, title: o.title ?? null, codec: o.codec ?? "subrip", external: false,
    });
    const subLangs = props.prefs.subtitle_languages.map(iso1).filter(Boolean);
    const want = subLangs[0] ?? mine;
    const a = {
      title: `A film in ${languageName(mine)}`,
      audio: [{ index: 1, language: mine, channels: 6, codec: "eac3", default: true }],
      subs: [sub(2, mine, { forced: true }), sub(3, mine), sub(4, mine, { title: "SDH" }), sub(5, want === mine ? "es" : want)],
    };
    const b = {
      title: `A film in ${languageName(foreign())}`,
      audio: [
        { index: 1, language: foreign(), channels: 6, codec: "eac3", default: true },
        { index: 2, language: mine, channels: 6, codec: "eac3", default: false },
      ],
      subs: [sub(3, want), sub(4, want, { title: "SDH" }), sub(5, want, { codec: "hdmv_pgs_subtitle" })],
    };
    return [a, b].map((s) => {
      const r = previewSelection(s.audio, s.subs, props.prefs);
      const au = s.audio.find((x) => x.index === r.audio);
      const sb = s.subs.find((x) => x.index === r.sub);
      return {
        title: s.title,
        audio: au ? languageName(au.language) : "Default track",
        subs: sb ? `${languageName(sb.language)}${sb.forced ? " (forced parts only)" : sb.title === "SDH" ? " SDH" : ""}` : "Off",
      };
    });
  });
  return (
    <div class="prefs__examples">
      <p class="eyebrow">With these settings</p>
      <For each={samples()}>
        {(s) => (
          <div class="prefs__example">
            <strong>{s.title}</strong>
            <dl>
              <dt><Icon name="audio" /> Audio</dt>
              <dd>{s.audio}</dd>
              <dt><Icon name="cc" /> Subtitles</dt>
              <dd classList={{ "is-off": s.subs === "Off" }}>{s.subs}</dd>
            </dl>
          </div>
        )}
      </For>
      <p class="prefs__fine">
        "Original language" uses the title's own language. Change tracks while watching and Reel offers to remember it.
      </p>
    </div>
  );
}
