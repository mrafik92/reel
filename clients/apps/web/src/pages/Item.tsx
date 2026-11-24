import { createEffect, createMemo, createResource, createSignal, For, on, Show, type JSX } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import {
  Icon, Row, Sheet, focusElement,
  audioLabel, codecName, channelsLabel, episodeCode, fileTags, formatBytes, formatRemaining, formatRuntime,
  hdrLabel, resolutionLabel, subtitleLabel,
  type ItemDetail, type ItemSummary, type MediaFile,
} from "@reel/ui";
import { api, invalidateLibrary } from "../api";
import { toast } from "../lib/toast";
import { usePageFocus } from "../lib/focus";
import { playHref, toPath } from "../lib/links";
import { previewSelection } from "../lib/tracks";
import { loadPrefs, playbackPrefs } from "../lib/prefs";
import { castFor, type CastTarget } from "../lib/casting";
import { requestCastSession, CastCancelled } from "../lib/casting/chromecast";
import { CastSheet } from "../components/casting/CastSheet";
import { TV } from "../lib/mode";
import { ErrorState } from "../components/States";
import { RatingsStrip, ReviewsSection, useReviews } from "../components/Reviews";
import { CastTile } from "../components/Cast";
import { href } from "../lib/mode";
import { itemActivity } from "../lib/downloads";
import { episodeTag } from "../lib/continue";
import { CastText, InfoLine, RoundAction, SeasonPicker, TitleSheet } from "../components/TitleSheet";
import { EpisodeList } from "../components/EpisodeList";

type Picker = "audio" | "sub" | "version" | "cast" | null;
/** Sheet value for "Automatic" (use my playback settings). */
const AUTO = -2;

export default function Item(): JSX.Element {
  const params = useParams<{ id: string }>();
  const [item, { refetch }] = createResource(() => Number(params.id), (id) => api.item(id));

  // Shows are "ready" once episodes are in, so focus lands on Play, not the cast row.
  const [bodyReady, setBodyReady] = createSignal(false);
  let el!: HTMLDivElement;
  usePageFocus(() => (!!item() && bodyReady()) || !!item.error, () => el);

  return (
    <div class="page detail" ref={el}>
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
  // Until you pick, the buttons preview what the server will choose from your playback
  // settings ("Auto"); a pick here travels to the player in the Play link.
  void loadPrefs();
  const [fileIdx, setFileIdx] = createSignal(0);
  const file = (): MediaFile | undefined => it().files[fileIdx()] ?? it().files[0];
  const defaultAudio = () => (file()?.audio.find((a) => a.default) ?? file()?.audio[0])?.index ?? -1;
  /** null = automatic */
  const [audio, setAudio] = createSignal<number | null>(null);
  const [sub, setSub] = createSignal<number | null>(null);
  const preview = createMemo(() => {
    const p = playbackPrefs();
    const f = file();
    return p && f ? previewSelection(f.audio, f.subtitles, p) : undefined;
  });
  const audioIdx = () => {
    const a = audio();
    return a != null && file()?.audio.some((x) => x.index === a) ? a : preview()?.audio ?? defaultAudio();
  };
  const subIdx = () => sub() ?? preview()?.sub ?? -1;
  const audioText = () => {
    const a = file()?.audio.find((x) => x.index === audioIdx());
    return a ? audioLabel(a).short : "Default";
  };
  const subText = () => {
    const s = file()?.subtitles.find((x) => x.index === subIdx());
    return s ? subtitleLabel(s).short : "Off";
  };
  const [picker, setPicker] = createSignal<Picker>(null);
  const trackOpts = () => ({ file: file()!.id, audio: audio(), sub: sub() });

  // ---- Play on… (TVs): the player starts straight on the target.
  const navigate = useNavigate();
  const castNow = () => castFor(`item:${it().id}`);
  const playOn = async (t: CastTarget) => {
    const target = t.kind === "dlna" ? `dlna:${t.id}` : t.kind;
    if (t.kind === "chromecast") {
      // Chrome's device picker needs this click; the player then loads onto the session.
      try {
        await requestCastSession();
      } catch (e) {
        if (!(e instanceof CastCancelled)) toast(e instanceof Error ? e.message : "Couldn't connect", "error");
        return;
      }
    }
    navigate(toPath(playHref(it(), { ...trackOpts(), cast: target })));
  };
  /** "From your playback settings: English SDH" — what Automatic resolves to right now. */
  const autoDetail = (kind: "audio" | "sub"): string => {
    const p = preview();
    if (!p) return "From your playback settings";
    if (kind === "audio") {
      const a = file()?.audio.find((x) => x.index === p.audio);
      return `From your playback settings: ${a ? audioLabel(a).short : "default track"}`;
    }
    const s = file()?.subtitles.find((x) => x.index === p.sub);
    return `From your playback settings: ${s ? subtitleLabel(s).short : "off"}`;
  };

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
  // Ratings & reviews exist for films and whole series (not seasons/episodes).
  const reviews = useReviews(() =>
    it().tmdb_id && (it().kind === "movie" || it().kind === "show")
      ? { media: it().kind === "show" ? "tv" : "movie", tmdbId: it().tmdb_id! }
      : undefined,
  );
  const activity = () => itemActivity(it().id);

  const epProgress = (ep: ItemSummary) => (ep.progress && ep.progress > 0.005 && ep.progress < 0.995 ? ep.progress : 0);
  const castNames = () => it().cast.map((c) => c.name);
  const jumpToCast = () => {
    const link = document.querySelector<HTMLElement>(".row--cast a, .row--cast [data-focusable]");
    if (link) focusElement(link);
  };

  const head = (
    <>
      <Show when={it().kind === "episode"}>
        <p class="eyebrow">{episodeCode(it())}</p>
      </Show>
      <h1 class="detail__title">{it().title}</h1>
      <div class="actions tsheet__actions" data-nav-group>
        <Show when={isShow()} fallback={
          <Show when={file()} fallback={<p class="note"><Icon name="alert" /> No playable file for this title yet.</p>}>
            <a class="btn btn--primary btn--lg tsheet__play" href={playHref(it(), trackOpts())} data-autofocus>
              <Icon name="play" />
              <span>{progress() ? "Resume" : "Play"}</span>
              <Show when={progress() && runtime()}>
                <small class="btn__aside">{formatRemaining(runtime()!, progress())}</small>
              </Show>
            </a>
            <Show when={progress()}>
              <RoundAction icon={<Icon name="replay" />} label="From the beginning" href={playHref(it(), { ...trackOpts(), fromStart: true })} />
            </Show>
          </Show>
        }>
          <Show when={nextUp()} fallback={<span class="btn btn--primary btn--lg tsheet__play is-disabled"><Icon name="play" /> <span>Loading episodes…</span></span>}>
            {(ep) => (
              <>
                <a class="btn btn--primary btn--lg tsheet__play" href={playHref(ep())} data-autofocus>
                  <Icon name="play" />
                  <span>{epProgress(ep()) ? "Resume" : "Play"} {episodeTag(ep())}</span>
                  <Show when={epProgress(ep()) && ep().runtime_s}>
                    <small class="btn__aside">{formatRemaining(ep().runtime_s!, epProgress(ep()))}</small>
                  </Show>
                </a>
                <Show when={epProgress(ep())}>
                  <RoundAction icon={<Icon name="replay" />} label="From the beginning" href={playHref(ep(), { fromStart: true })} />
                </Show>
              </>
            )}
          </Show>
        </Show>
        <RoundAction
          icon={<Icon name={played() ? "eyeoff" : "check"} />}
          label={played() ? "Mark unwatched" : "Mark watched"}
          pressed={played()}
          disabled={savingPlayed()}
          onClick={() => void togglePlayed()}
        />
        <Show when={!TV && !isShow() && file()}>
          <RoundAction
            icon={<Icon name="cast" />}
            label={castNow() ? `On ${castNow()!.remote.device()}` : "Play on…"}
            onClick={() => setPicker("cast")}
          />
        </Show>
      </div>
    </>
  );

  return (
    <TitleSheet backdrop={it().backdrop_url} head={head}>
      <section class="tinfo" data-nav-anchor>
        <div class="tinfo__main">
          <p class="meta">
            <Show when={it().year}><span>{it().year}</span></Show>
            <Show when={it().content_rating}><span class="badge">{it().content_rating}</span></Show>
            <Show when={!isShow() && runtime()}><span>{formatRuntime(runtime())}</span></Show>
            <Show when={it().kind === "show" && it().children_count}>
              <span>{it().children_count} {it().children_count === 1 ? "season" : "seasons"}</span>
            </Show>
            <For each={tags()}>{(t) => <span class="tag" classList={{ "tag--hdr": /HDR|Dolby Vision|HLG/.test(t) }}>{t}</span>}</For>
            <Show when={it().rating}>
              <span class="meta__rating"><Icon name="star" filled /> {it().rating!.toFixed(1)}</span>
            </Show>
            <Show when={played()}><span class="meta__watched"><Icon name="check" /> Watched</span></Show>
          </p>
          <RatingsStrip data={reviews()} />
          <Show when={activity()}>
            {(a) => (
              <p class="detail__activity">
                <span class="pulse" aria-hidden="true" />
                {a().state === "downloading" ? `Downloading${a().count > 1 ? ` ${a().count} files` : ""} · ${Math.floor(a().progress * 100)}%` : a().state === "importing" ? "Importing new files" : a().state === "queued" ? "Queued for download" : a().state === "stalled" ? "Download stalled" : "Download failed"}
                <a href={href("/downloads")}>See downloads</a>
              </p>
            )}
          </Show>
          <Show when={it().overview}>
            <p class="detail__overview">{it().overview}</p>
          </Show>
          <Show when={!isShow() && file()}>
            <div class="tinfo__selects" data-nav-group>
              <button type="button" class="btn btn--sm btn--select" onClick={() => setPicker("audio")} disabled={!file()!.audio.length}>
                <Icon name="audio" /> <span class="btn__label">Audio</span> <span class="btn__value">{audioText()}</span>
              </button>
              <button type="button" class="btn btn--sm btn--select" onClick={() => setPicker("sub")}>
                <Icon name="cc" /> <span class="btn__label">Subtitles</span> <span class="btn__value">{subText()}</span>
                <Show when={sub() == null && preview()}><small class="btn__tag">Auto</small></Show>
              </button>
              <Show when={it().files.length > 1}>
                <button type="button" class="btn btn--sm btn--select" onClick={() => setPicker("version")}>
                  <Icon name="layers" /> <span class="btn__label">Version</span>
                  <span class="btn__value">{tags().slice(0, 2).join(" ")}</span>
                </button>
              </Show>
            </div>
          </Show>
        </div>
        <div class="tinfo__side">
          <Show when={it().cast.length}>
            <InfoLine label="Cast">
              {/* On TV the names aren't extra focus stops: the cast row below has them. */}
              <Show when={!TV} fallback={<CastText names={castNames().slice(0, 4)} />}>
                <For each={it().cast.slice(0, 4)}>
                  {(c, i) => (
                    <>
                      {i() > 0 ? ", " : ""}
                      <Show when={c.person_tmdb_id} fallback={c.name}>
                        <a class="tinfo__link" href={href(`/person/${c.person_tmdb_id}`)}>{c.name}</a>
                      </Show>
                    </>
                  )}
                </For>
                <Show when={it().cast.length > 4}>
                  {", "}
                  <button type="button" class="tinfo__more" onClick={jumpToCast}>more</button>
                </Show>
              </Show>
            </InfoLine>
          </Show>
          <Show when={it().genres.length}>
            <InfoLine label="Genres">{it().genres.join(", ")}</InfoLine>
          </Show>
        </div>
      </section>

      <Show when={isShow()}>
        <section class="tepisodes" data-nav-anchor>
          <header class="tepisodes__head">
            <h2 class="section-title">Episodes</h2>
            <Show when={it().kind === "show" && seasons() && seasons()!.length}>
              <SeasonPicker
                seasons={seasons()!.map((x) => ({ value: x.id, label: x.title }))}
                selected={seasonId() ?? seasons()![0]!.id}
                onSelect={(v) => setSeasonId(v)}
              />
            </Show>
          </header>
          <Show when={episodes()} fallback={<div class="episodes-loading" />} keyed>
            {(eps) => <EpisodeList episodes={eps} nextId={nextUp()?.id} />}
          </Show>
        </section>
      </Show>

      <Show when={it().cast.length}>
        <Row title="Cast" variant="cast">
          <For each={it().cast.slice(0, 20)}>
            {(c) => (
              <CastTile c={c} />
            )}
          </For>
        </Row>
      </Show>

      <ReviewsSection data={reviews()} />

      <section class="tabout" data-nav-anchor>
        <h2 class="section-title">About {it().title}</h2>
        <Show when={it().genres.length}><InfoLine label="Genres">{it().genres.join(", ")}</InfoLine></Show>
        <Show when={it().content_rating}><InfoLine label="Maturity rating">{it().content_rating}</InfoLine></Show>
        <Show when={it().cast.length}><InfoLine label="Cast"><CastText names={castNames()} /></InfoLine></Show>
      </section>

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
          selected={audio() ?? AUTO}
          options={[{ value: AUTO, title: "Automatic", detail: autoDetail("audio") }].concat(
            file()!.audio.map((a) => ({ value: a.index, ...audioLabel(a) })),
          )}
          onSelect={(v) => setAudio(v === AUTO ? null : v)}
          onClose={() => setPicker(null)}
        />
      </Show>
      <Show when={picker() === "sub" && file()}>
        <Sheet
          title="Subtitles"
          selected={sub() ?? AUTO}
          options={[
            { value: AUTO, title: "Automatic", detail: autoDetail("sub") },
            { value: -1, title: "Off", detail: "" },
          ].concat(file()!.subtitles.map((s) => ({ value: s.index, ...subtitleLabel(s) })))}
          onSelect={(v) => setSub(v === AUTO ? null : v)}
          footer={
            <a class="sheet__link" href={href("/settings/playback")}>
              <Icon name="gear" /> Subtitle & audio preferences
            </a>
          }
          onClose={() => setPicker(null)}
        />
      </Show>
      <Show when={picker() === "cast"}>
        <CastSheet airplay={false} current={castNow()?.target.id} onPick={(t) => void playOn(t)} onClose={() => setPicker(null)} />
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
    </TitleSheet>
  );
}

function DetailSkeleton(): JSX.Element {
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
