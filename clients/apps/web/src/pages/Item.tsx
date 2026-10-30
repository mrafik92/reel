import { createEffect, createMemo, createResource, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useParams } from "@solidjs/router";
import {
  Backdrop, Icon, Img, PosterCard, Row, Sheet,
  audioLabel, codecName, channelsLabel, episodeCode, fileTags, formatBytes, formatRemaining, formatRuntime,
  hdrLabel, resolutionLabel, subtitleLabel,
  type ItemDetail, type ItemSummary, type MediaFile,
} from "@reel/ui";
import { api, invalidateLibrary } from "../api";
import { toast } from "../lib/toast";
import { usePageFocus } from "../lib/focus";
import { playHref } from "../lib/links";
import { getTrackPrefs, setTrackPrefs } from "../lib/playback";
import { ErrorState } from "../components/States";

type Picker = "audio" | "sub" | "version" | null;

export default function Item(): JSX.Element {
  const params = useParams<{ id: string }>();
  const [item, { refetch }] = createResource(() => Number(params.id), (id) => api.item(id));

  // Shows are "ready" once episodes are in, so focus lands on Play, not the cast row.
  const [bodyReady, setBodyReady] = createSignal(false);
  let el!: HTMLDivElement;
  usePageFocus(() => (!!item() && bodyReady()) || !!item.error, () => el);

  // Dim the backdrop once the viewer moves past the hero into rows.
  const [scrolled, setScrolled] = createSignal(false);
  onMount(() => {
    const onScroll = () => setScrolled(window.pageYOffset > window.innerHeight * 0.3);
    window.addEventListener("scroll", onScroll, { passive: true });
    onCleanup(() => window.removeEventListener("scroll", onScroll));
  });

  return (
    <div class="page detail" ref={el} classList={{ "is-scrolled": scrolled() }}>
      <Backdrop src={item()?.backdrop_url} class="page-backdrop" />
      <div class="page-scrim page-scrim--detail" aria-hidden="true" />
      <Show when={!item.error} fallback={<ErrorState error={item.error} onRetry={refetch} />}>
        <Show when={item()} fallback={<DetailSkeleton />}>
          {(it) => <DetailBody item={it()} onReady={() => setBodyReady(true)} />}
        </Show>
      </Show>
    </div>
  );
}

function DetailBody(props: { item: ItemDetail; onReady: () => void }): JSX.Element {
  const it = () => props.item;
  const isShow = () => it().kind === "show" || it().kind === "season";

  // ---- Versions & tracks (movies / episodes)
  const prefs = getTrackPrefs(props.item.id);
  const [fileIdx, setFileIdx] = createSignal(0);
  const file = (): MediaFile | undefined => it().files[fileIdx()] ?? it().files[0];
  const defaultAudio = () => (file()?.audio.find((a) => a.default) ?? file()?.audio[0])?.index ?? -1;
  const [audio, setAudio] = createSignal<number>(prefs.audio ?? -2);
  const [sub, setSub] = createSignal<number>(prefs.sub ?? -1);
  const audioIdx = () => (audio() === -2 || !file()?.audio.some((a) => a.index === audio()) ? defaultAudio() : audio());
  const audioText = () => {
    const a = file()?.audio.find((x) => x.index === audioIdx());
    return a ? audioLabel(a).title + " · " + channelsLabel(a.channels) : "Default";
  };
  const subText = () => {
    const s = file()?.subtitles.find((x) => x.index === sub());
    return s ? subtitleLabel(s).title : "Off";
  };
  const [picker, setPicker] = createSignal<Picker>(null);

  // ---- Seasons & episodes (shows)
  const [seasons] = createResource(
    () => (it().kind === "show" ? it().id : undefined),
    (id) => api.children(id),
  );
  const [seasonId, setSeasonId] = createSignal<number>();
  createEffect(
    on(seasons, (ss) => {
      if (!ss || !ss.length || seasonId() != null) return;
      setSeasonId((ss.find((s) => !s.played) ?? ss[0]!).id);
    }),
  );
  const [episodes] = createResource(
    () => (it().kind === "season" ? it().id : seasonId()),
    (id) => api.children(id),
  );
  createEffect(() => {
    if (!isShow() || episodes() || seasons.error || episodes.error || (seasons() && !seasons()!.length)) props.onReady();
  });
  const nextUp = createMemo<ItemSummary | undefined>(() => {
    const eps = episodes();
    if (!eps || !eps.length) return undefined;
    return eps.find((e) => e.progress) ?? eps.find((e) => !e.played) ?? eps[0];
  });

  let seasonTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => seasonTimer && clearTimeout(seasonTimer));
  const pickSeason = (id: number, delay: number) => {
    if (seasonTimer) clearTimeout(seasonTimer);
    seasonTimer = setTimeout(() => setSeasonId(id), delay);
  };

  // Focused episode → overview (fetched once the viewer settles).
  const [epFocus, setEpFocus] = createSignal<ItemSummary>();
  const [epDetail] = createResource(epFocus, (e) => new Promise<ItemDetail>((res, rej) => {
    setTimeout(() => api.item(e.id).then(res, rej), 300);
  }));

  const [played, setPlayedLocal] = createSignal(props.item.played);
  const [savingPlayed, setSavingPlayed] = createSignal(false);
  const togglePlayed = async () => {
    if (savingPlayed()) return;
    const next = !played();
    setSavingPlayed(true);
    setPlayedLocal(next);
    try {
      await api.setPlayed(it().id, next);
      invalidateLibrary();
    } catch (e) {
      setPlayedLocal(!next);
      toast(e instanceof Error ? e.message : "Couldn't update", "error");
    } finally {
      setSavingPlayed(false);
    }
  };
  const progress = () => (!played() && it().progress && it().progress! > 0.005 ? it().progress! : 0);
  const runtime = () => it().runtime_s ?? file()?.duration_s;
  const tags = () => fileTags(file());

  return (
    <>
      <section class="detail__hero" data-nav-anchor style={{ "--nav-anchor-top": "0px" }}>
        <p class="eyebrow">
          {it().kind === "show" ? "Series" : it().kind === "season" ? "Season" : it().kind === "episode" ? episodeCode(it()) : "Film"}
        </p>
        <h1 class="detail__title">{it().title}</h1>
        <p class="meta">
          <Show when={it().year}><span>{it().year}</span></Show>
          <Show when={it().content_rating}><span class="badge">{it().content_rating}</span></Show>
          <Show when={!isShow() && runtime()}><span>{formatRuntime(runtime())}</span></Show>
          <Show when={it().kind === "show" && it().children_count}>
            <span>{it().children_count} {it().children_count === 1 ? "season" : "seasons"}</span>
          </Show>
          <Show when={it().genres.length}><span>{it().genres.slice(0, 3).join(" / ")}</span></Show>
          <Show when={it().rating}>
            <span class="meta__rating"><Icon name="star" filled /> {it().rating!.toFixed(1)}</span>
          </Show>
          <Show when={played()}><span class="meta__watched"><Icon name="check" /> Watched</span></Show>
        </p>
        <Show when={tags().length}>
          <ul class="tags" aria-label="Video and audio">
            <For each={tags()}>{(t) => <li class="tag" classList={{ "tag--hdr": /HDR|Dolby Vision|HLG/.test(t) }}>{t}</li>}</For>
          </ul>
        </Show>
        <Show when={it().overview}>
          <p class="detail__overview">{it().overview}</p>
        </Show>

        <div class="actions" data-nav-group>
          <Show when={isShow()} fallback={
            <Show when={file()} fallback={<p class="note"><Icon name="alert" /> No playable file for this title yet.</p>}>
              <a class="btn btn--primary btn--lg" href={playHref(it(), { file: file()!.id })} data-autofocus>
                <Icon name="play" />
                <span>{progress() ? "Resume" : "Play"}</span>
                <Show when={progress() && runtime()}>
                  <small class="btn__aside">{formatRemaining(runtime()!, progress())}</small>
                </Show>
              </a>
              <Show when={progress()}>
                <a class="btn" href={playHref(it(), { file: file()!.id, fromStart: true })}>
                  <Icon name="restart" /> <span>From the beginning</span>
                </a>
              </Show>
              <button type="button" class="btn btn--select" onClick={() => setPicker("audio")} disabled={!file()!.audio.length}>
                <Icon name="audio" /> <span class="btn__label">Audio</span> <span class="btn__value">{audioText()}</span>
              </button>
              <button type="button" class="btn btn--select" onClick={() => setPicker("sub")}>
                <Icon name="cc" /> <span class="btn__label">Subtitles</span> <span class="btn__value">{subText()}</span>
              </button>
              <Show when={it().files.length > 1}>
                <button type="button" class="btn btn--select" onClick={() => setPicker("version")}>
                  <Icon name="layers" /> <span class="btn__label">Version</span>
                  <span class="btn__value">{tags().slice(0, 2).join(" ")}</span>
                </button>
              </Show>
            </Show>
          }>
            <Show when={nextUp()} fallback={<span class="btn btn--primary btn--lg is-disabled"><Icon name="play" /> <span>Loading episodes…</span></span>}>
              {(ep) => (
                <a class="btn btn--primary btn--lg" href={playHref(ep())} data-autofocus>
                  <Icon name="play" />
                  <span>{ep().progress ? "Resume" : "Play"} {episodeCode(ep())}</span>
                </a>
              )}
            </Show>
          </Show>
          <button type="button" class="btn" aria-pressed={played()} aria-disabled={savingPlayed() ? "true" : undefined} onClick={() => void togglePlayed()}>
            <Icon name={played() ? "eyeoff" : "check"} />
            <span>{played() ? "Mark unwatched" : "Mark watched"}</span>
          </button>
        </div>
      </section>

      <Show when={it().kind === "show" && seasons() && seasons()!.length}>
        <section class="seasons" data-nav-anchor>
          <nav class="tabs tabs--seasons" aria-label="Seasons" data-nav-group>
            <For each={seasons()}>
              {(s) => (
                <button
                  type="button"
                  class="tab"
                  aria-current={s.id === seasonId() ? "true" : undefined}
                  data-nav-default={s.id === seasonId() ? "" : undefined}
                  data-fid={s.id}
                  onFocus={() => pickSeason(s.id, 220)}
                  onClick={() => pickSeason(s.id, 0)}
                >
                  {s.title}
                </button>
              )}
            </For>
          </nav>
        </section>
      </Show>

      <Show when={isShow()}>
        <Show when={episodes()} fallback={<div class="episodes-loading" />}>
          {(eps) => (
            <>
              <Row title={`${eps().length} episodes`} variant="landscape" class="row--episodes">
                <For each={eps()}>
                  {(ep) => (
                    <PosterCard
                      item={ep}
                      variant="landscape"
                      href={playHref(ep)}
                      subtitle={`${/^Episode \d+$/i.test(ep.title) ? episodeCode(ep) : `Episode ${ep.episode_no ?? ""}`}${ep.played ? " · Watched" : ""}`}
                      onFocus={setEpFocus}
                    />
                  )}
                </For>
              </Row>
              <div class="episode-note" aria-live="polite">
                <Show when={epFocus() && epDetail() && epDetail()!.id === epFocus()!.id && epDetail()}>
                  {(d) => (
                    <>
                      <p class="eyebrow">{episodeCode(d())} · {formatRuntime(d().runtime_s)}</p>
                      <h3>{d().title}</h3>
                      <p>{d().overview}</p>
                    </>
                  )}
                </Show>
              </div>
            </>
          )}
        </Show>
      </Show>

      <Show when={it().cast.length}>
        <Row title="Cast" variant="cast">
          <For each={it().cast.slice(0, 20)}>
            {(c) => (
              <div class="person" tabindex="0" data-focusable data-fid={"p:" + c.name}>
                <Img
                  class="person__photo"
                  src={c.photo_url}
                  width={185}
                  fallback={<span class="person__initials">{c.name.split(" ").map((w) => w.charAt(0)).join("").slice(0, 2)}</span>}
                />
                <span class="person__name">{c.name}</span>
                <Show when={c.character}><span class="person__role">{c.character}</span></Show>
              </div>
            )}
          </For>
        </Row>
      </Show>

      <Show when={it().files.length}>
        <section class="tech" data-nav-anchor>
          <h2 class="section-title">File &amp; streams</h2>
          <For each={it().files}>
            {(f, i) => (
              <div class="tech__file" tabindex="0" data-focusable data-fid={"f:" + f.id} classList={{ "is-current": i() === fileIdx() }}>
                <div class="tech__summary">
                  <span class="tech__ver">{it().files.length > 1 ? `Version ${i() + 1}` : "Source"}</span>
                  <Show when={f.item_ids && f.item_ids.length > 1}>
                    <span class="tag">{f.item_ids!.length} episodes in one file</span>
                  </Show>
                  <span>{f.container.toUpperCase()}</span>
                  <span>{formatBytes(f.size)}</span>
                  <span>{formatRuntime(f.duration_s)}</span>
                </div>
                <dl class="tech__grid">
                  <Show when={f.video}>
                    {(v) => (
                      <>
                        <dt>Video</dt>
                        <dd>
                          {resolutionLabel(v())} · {codecName(v().codec)} · {v().width}×{v().height} · {v().bit_depth}-bit
                          {hdrLabel(v().hdr) ? " · " + hdrLabel(v().hdr) : ""}
                        </dd>
                      </>
                    )}
                  </Show>
                  <dt>Audio</dt>
                  <dd>
                    <ul>
                      <For each={f.audio}>
                        {(a) => <li>{audioLabel(a).title} <span>{audioLabel(a).detail}{a.default ? " · Default" : ""}</span></li>}
                      </For>
                    </ul>
                  </dd>
                  <dt>Subtitles</dt>
                  <dd>
                    <Show when={f.subtitles.length} fallback="None">
                      <ul>
                        <For each={f.subtitles}>
                          {(s) => <li>{subtitleLabel(s).title} <span>{subtitleLabel(s).detail}</span></li>}
                        </For>
                      </ul>
                    </Show>
                  </dd>
                </dl>
              </div>
            )}
          </For>
        </section>
      </Show>

      <Show when={picker() === "audio" && file()}>
        <Sheet
          title="Audio"
          selected={audioIdx()}
          options={file()!.audio.map((a) => ({ value: a.index, ...audioLabel(a) }))}
          onSelect={(v) => {
            setAudio(v);
            setTrackPrefs(it().id, { audio: v });
          }}
          onClose={() => setPicker(null)}
        />
      </Show>
      <Show when={picker() === "sub" && file()}>
        <Sheet
          title="Subtitles"
          selected={sub()}
          options={[{ value: -1, title: "Off" }].concat(
            file()!.subtitles.map((s) => ({ value: s.index, ...subtitleLabel(s) })),
          )}
          onSelect={(v) => {
            setSub(v);
            setTrackPrefs(it().id, { sub: v });
          }}
          onClose={() => setPicker(null)}
        />
      </Show>
      <Show when={picker() === "version"}>
        <Sheet
          title="Version"
          selected={fileIdx()}
          options={it().files.map((f, i) => ({
            value: i,
            title: fileTags(f).slice(0, 3).join(" · ") || `Version ${i + 1}`,
            detail: `${f.container.toUpperCase()} · ${formatBytes(f.size)}`,
          }))}
          onSelect={(v) => setFileIdx(v)}
          onClose={() => setPicker(null)}
        />
      </Show>
    </>
  );
}

function DetailSkeleton(): JSX.Element {
  return (
    <section class="detail__hero" aria-busy="true" aria-label="Loading">
      <span class="skel skel--text" style={{ width: "6rem" }} />
      <span class="skel skel--title" />
      <span class="skel skel--text" style={{ width: "22rem" }} />
      <span class="skel skel--text" style={{ width: "34rem" }} />
      <span class="skel skel--text" style={{ width: "30rem" }} />
    </section>
  );
}
