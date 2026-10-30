import { createEffect, createMemo, createResource, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { useNavigate, useSearchParams } from "@solidjs/router";
import { focusElement, Icon, onBack, onRemoteKey } from "@reel/ui";
import "../styles/live.css";
import { iptv, type IptvChannel } from "../api/iptv";
import { href } from "../lib/mode";
import { isAdmin } from "../lib/auth";
import { toast } from "../lib/toast";
import { ErrorState } from "../components/States";
import { ChannelList, type ChannelListApi } from "../components/live/ChannelList";
import { NowPlaying, NumberEntry, ZapStats } from "../components/live/Overlays";
import { Filters } from "../components/live/Filters";
import { createTuner } from "../components/live/tuner";
import {
  channelById, channelByNumber, channels, channelsError, filter, filtered, fmtRange, fmtTime, lastChannel, loadChannels,
  rememberChannel, toggleFavorite, nowSec, clock, ensureNow, filterKey,
} from "../components/live/store";

const STATS_KEY = "reel.live.stats";
const ZAP_DEBOUNCE = 320;
const DIGIT_WAIT = 1500;

export default function LiveTV(): JSX.Element {
  onMount(() => {
    void loadChannels();
    document.documentElement.classList.add("is-live-page");
    onCleanup(() => document.documentElement.classList.remove("is-live-page"));
  });
  return (
    <div class="live-page">
      <Switch>
        <Match when={channelsError()}>
          <div class="page"><ErrorState error={channelsError()} onRetry={() => void loadChannels(true)} title="Couldn't load your channels" /></div>
        </Match>
        <Match when={!channels()}>
          <LiveSkeleton />
        </Match>
        <Match when={channels()!.length === 0}>
          <LiveEmpty />
        </Match>
        <Match when={channels()!.length > 0}>
          <Watch />
        </Match>
      </Switch>
    </div>
  );
}

function Watch(): JSX.Element {
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams<{ ch?: string }>();
  const [full, setFull] = createSignal(false);
  const [mini, setMini] = createSignal(false);
  const [osd, setOsd] = createSignal(false);
  const [stats, setStats] = createSignal(readFlag(STATS_KEY));
  const [digits, setDigits] = createSignal("");
  const [missing, setMissing] = createSignal(false);
  const [pending, setPending] = createSignal<IptvChannel>();
  let video!: HTMLVideoElement;
  let screen!: HTMLDivElement;
  let listApi: ChannelListApi | undefined;
  let miniApi: ChannelListApi | undefined;
  const tuner = createTuner(() => video);

  const tuned = () => channelById(tuner.channelId());
  /** What the overlays describe: the zap target while it's pending, else the tuned channel. */
  const shown = () => pending() ?? tuned();
  // Tizen's AVPlay draws on a full-screen plane behind the page: no windowed preview there.
  const isFull = () => full() || (tuner.nativePlane() && tuner.phase() !== "idle");
  const list = createMemo(() => {
    const f = filtered();
    const id = tuner.channelId();
    return id == null || f.some((c) => c.id === id) || !f.length ? (f.length ? f : channels()!) : channels()!;
  });
  const maxDigits = createMemo(() => {
    let m = 0;
    (channels() ?? []).forEach((c) => c.number != null && c.number > m && (m = c.number));
    return Math.min(4, Math.max(1, String(m).length));
  });

  // Overlays need now/next even for channels that aren't on screen in the list.
  createEffect(() => {
    const c = shown();
    if (c) ensureNow([c.id]);
  });

  // ---- tuning --------------------------------------------------------------------
  let osdTimer: ReturnType<typeof setTimeout> | undefined;
  const showOsd = (ms = 5000) => {
    setOsd(true);
    if (osdTimer) clearTimeout(osdTimer);
    osdTimer = setTimeout(() => setOsd(false), ms);
  };
  const tuneNow = (c: IptvChannel) => {
    rememberChannel(c.id);
    setPending(undefined);
    showOsd();
    void tuner.tune(c);
  };

  let zapTimer: ReturnType<typeof setTimeout> | undefined;
  const zap = (dir: 1 | -1) => {
    const l = list();
    if (!l.length) return;
    const base = pending() ?? tuned();
    const i = base ? l.findIndex((c) => c.id === base.id) : -1;
    const next = l[(i + dir + l.length) % l.length]!;
    setPending(next);
    showOsd();
    listApi?.reveal(next.id, !isFull());
    if (zapTimer) clearTimeout(zapTimer);
    // Coalesce held keys into one tune once the viewer settles.
    zapTimer = setTimeout(() => tuneNow(next), ZAP_DEBOUNCE);
  };

  const pick = (c: IptvChannel) => {
    if (c.id === tuner.channelId() && tuner.phase() !== "error") enterFull();
    else tuneNow(c);
  };

  // ---- number entry ----------------------------------------------------------------
  let digitTimer: ReturnType<typeof setTimeout> | undefined;
  const clearDigits = () => {
    if (digitTimer) clearTimeout(digitTimer);
    setDigits("");
    setMissing(false);
  };
  const commitDigits = () => {
    if (digitTimer) clearTimeout(digitTimer);
    const n = Number(digits());
    const c = channelByNumber(n);
    if (!c) {
      setMissing(true);
      digitTimer = setTimeout(clearDigits, 1400);
      return;
    }
    clearDigits();
    if (zapTimer) clearTimeout(zapTimer);
    listApi?.reveal(c.id, !isFull());
    if (c.id !== tuner.channelId()) tuneNow(c);
  };
  const pushDigit = (d: number) => {
    if (missing()) clearDigits();
    const s = (digits().length >= maxDigits() ? "" : digits()) + String(d);
    setDigits(s);
    if (digitTimer) clearTimeout(digitTimer);
    if (s.length >= maxDigits()) digitTimer = setTimeout(commitDigits, 250);
    else digitTimer = setTimeout(commitDigits, DIGIT_WAIT);
  };
  const digitTarget = () => (digits() && !missing() ? channelByNumber(Number(digits())) : undefined);

  // ---- modes -------------------------------------------------------------------------
  const enterFull = () => {
    setFull(true);
    showOsd();
    requestAnimationFrame(() => screen?.focus({ preventScroll: true }));
  };
  const exitFull = () => {
    setFull(false);
    setMini(false);
    setOsd(false);
    if (tuner.nativePlane()) tuner.stop();
    const id = tuner.channelId();
    requestAnimationFrame(() => id != null && listApi?.reveal(id, true));
  };
  const openMini = () => {
    setMini(true);
    setOsd(false);
    requestAnimationFrame(() => {
      const id = (pending() ?? tuned())?.id;
      if (id != null) miniApi?.reveal(id, true);
    });
  };
  const closeMini = () => {
    setMini(false);
    requestAnimationFrame(() => screen?.focus({ preventScroll: true }));
  };
  const toggleStats = () => {
    const v = !stats();
    setStats(v);
    writeFlag(STATS_KEY, v);
  };
  const fav = async (c = tuned()) => {
    if (!c) return;
    try {
      await toggleFavorite(c);
      toast(c.favorite ? `Removed ${c.name} from favourites` : `Added ${c.name} to favourites`, "success", 2000);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't update favourites", "error");
    }
  };

  // ---- first tune: ?ch= from the guide, else the last channel, else the top of the list ----
  let started = false;
  createEffect(() => {
    const l = channels();
    if (started || !l || !l.length) return;
    started = true;
    const want = Number(params.ch) || undefined;
    const c = channelById(want) ?? channelById(lastChannel()) ?? list()[0] ?? l[0]!;
    tuneNow(c);
    if (want) {
      setParams({ ch: undefined }, { replace: true });
      enterFull();
    } else requestAnimationFrame(() => listApi?.reveal(c.id, true));
  });
  // Guide → Watch while already on /live.
  createEffect(on(() => params.ch, (ch) => {
    if (!started || !ch) return;
    const c = channelById(Number(ch));
    setParams({ ch: undefined }, { replace: true });
    if (c) {
      if (c.id !== tuner.channelId()) tuneNow(c);
      enterFull();
    }
  }, { defer: true }));

  // ---- keys ------------------------------------------------------------------------------
  onMount(() => {
    const offKeys = onRemoteKey((e) => {
      const k = e.key;
      if (k === "digit") {
        pushDigit(e.digit ?? 0);
        return true;
      }
      if (k === "channelUp" || k === "channelDown") {
        if (mini()) setMini(false);
        zap(k === "channelUp" ? 1 : -1);
        return true;
      }
      if (k === "blue") {
        toggleStats();
        return true;
      }
      if (k === "yellow") {
        void fav();
        return true;
      }
      if (k === "info") {
        if (isFull()) (osd() ? setOsd(false) : showOsd(8000));
        else toggleStats();
        return true;
      }
      if (k === "enter" && digits()) {
        commitDigits();
        return true;
      }
      if (!isFull() || mini()) return false;
      switch (k) {
        case "up": zap(1); return true;
        case "down": zap(-1); return true;
        case "left": openMini(); return true;
        case "right": showOsd(8000); return true;
        case "enter":
          if (tuner.error()?.retry) tuner.retry();
          else openMini();
          return true;
        case "stop": exitFull(); return true;
      }
      return false;
    });
    const offBack = onBack(() => {
      if (digits()) {
        clearDigits();
        return true;
      }
      if (mini()) {
        closeMini();
        return true;
      }
      if (isFull()) {
        exitFull();
        return true;
      }
      return false;
    });
    // Desktop shortcuts.
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
      if (e.key === "f" || e.key === "F") (isFull() ? exitFull() : enterFull());
      else if (e.key === "i" || e.key === "I") toggleStats();
      else if (e.key === "g" || e.key === "G") navigate("/live/guide");
      else if (e.key === "m" || e.key === "M") void fav();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    const onHide = () => tuner.stop(true);
    window.addEventListener("pagehide", onHide);
    onCleanup(() => {
      offKeys();
      offBack();
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pagehide", onHide);
      [osdTimer, zapTimer, digitTimer].forEach((t) => t && clearTimeout(t));
      tuner.dispose();
    });
  });

  let lastMove = 0;
  const onPointer = () => {
    if (!isFull()) return;
    const n = Date.now();
    if (n - lastMove < 250) return;
    lastMove = n;
    showOsd(3500);
  };

  return (
    <div class="live" classList={{ "is-full": isFull(), "is-native": tuner.nativePlane(), "has-mini": mini() }}>
      <aside class="live__side" aria-label="Channels">
        <header class="live__head">
          <div>
            <p class="eyebrow eyebrow--accent">Live TV</p>
            <h1 class="live__title">
              Channels <span class="live__count">{list().length}</span>
            </h1>
          </div>
          <a class="btn btn--sm" href={href("/live/guide")}>
            <Icon name="grid" /> <span>Guide</span>
          </a>
        </header>
        <Filters />
        <ChannelList
          class="live__list"
          channels={filtered()}
          resetKey={filterKey(filter())}
          activeId={tuner.channelId()}
          pendingId={pending()?.id ?? digitTarget()?.id}
          onPick={pick}
          controller={(a) => (listApi = a)}
          rowRem={4.6}
          empty={
            <div class="chlist__empty">
              <Icon name="star" />
              <p>{filter().kind === "favorites" ? "No favourites yet. Press the yellow button (or M) while watching to add one." : "No channels in this group."}</p>
            </div>
          }
        />
      </aside>

      <section class="live__stage" data-nav-scroll="y">
        <div
          ref={screen}
          class="live__screen"
          classList={{ "is-osd": osd() || !!pending(), "is-error": !!tuner.error() }}
          tabindex={isFull() ? -1 : 0}
          data-focusable={isFull() ? undefined : ""}
          aria-label={isFull() ? `Watching ${tuned()?.name ?? ""}` : "Watch full screen"}
          onClick={() => (isFull() ? (osd() ? setOsd(false) : showOsd()) : enterFull())}
          onDblClick={() => (isFull() ? exitFull() : enterFull())}
          onMouseMove={onPointer}
        >
          <video ref={video} class="live__video" playsinline autoplay />
          <div class="live__vignette" aria-hidden="true" />

          <Show when={tuner.phase() === "tuning" && !tuner.error()}>
            <div class="live__tuning" aria-live="polite">
              <span class="spinner" />
              <span>Tuning {tuned()?.name}…</span>
            </div>
          </Show>
          <Show when={tuner.phase() === "playing" && tuner.buffering()}>
            <div class="live__tuning"><span class="spinner" /></div>
          </Show>

          <Show when={tuner.error()}>
            {(e) => (
              <div class="live__error" role="alert" classList={{ [`is-${e().kind}`]: true }} onClick={(ev) => ev.stopPropagation()}>
                <Icon name="alert" />
                <h2>{e().title}</h2>
                <p>{e().body}</p>
                <Show when={!isFull()} fallback={<p class="live__hint">{e().retry ? "OK to try again · ▲▼ another channel" : "▲▼ another channel"}</p>}>
                  <div class="actions actions--center">
                    <Show when={e().retry}>
                      <button type="button" class="btn btn--primary btn--sm" onClick={() => tuner.retry()}><Icon name="retry" /> <span>Try again</span></button>
                    </Show>
                  </div>
                </Show>
              </div>
            )}
          </Show>

          <Show when={tuner.muted()}>
            <button type="button" class="live__unmute" onClick={(ev) => { ev.stopPropagation(); tuner.unmute(); }}>
              <Icon name="audio" /> <span>Sound is off — click to unmute</span>
            </button>
          </Show>

          <Show when={isFull()}>
            <div class="live__top" classList={{ "is-on": osd() }} onClick={(ev) => ev.stopPropagation()}>
              <button type="button" class="iconbtn live__exit" aria-label="Back to channels" onClick={exitFull}><Icon name="back" /></button>
              <span class="live__clock">{fmtTime(clock())}</span>
              <span class="live__hints">
                <span><kbd>▲▼</kbd> channel</span>
                <span><kbd>OK</kbd> mini guide</span>
                <span><kbd>0–9</kbd> number</span>
              </span>
              <button type="button" class="iconbtn" aria-label="Channel list" onClick={openMini}><Icon name="layers" /></button>
              <a class="iconbtn" aria-label="Guide" href={href("/live/guide")}><Icon name="grid" /></a>
            </div>
            <Show when={(osd() || pending()) && shown()}>
              {(c) => (
                <div class="live__osd" onClick={(ev) => ev.stopPropagation()}>
                  <NowPlaying channel={c()} variant="overlay" tuning={!!pending() || tuner.phase() === "tuning"} />
                </div>
              )}
            </Show>
          </Show>

          <Show when={digits()}>
            <NumberEntry digits={digits()} max={maxDigits()} target={digitTarget()} missing={missing()} />
          </Show>

          <Show when={stats()}>
            <ZapStats samples={tuner.samples()} mode={tuner.session()?.mode} hls={tuner.session()?.hls} onClose={toggleStats} />
          </Show>
        </div>

        <Show when={!isFull() && shown()}>
          {(c) => (
            <div class="live__info">
              <NowPlaying channel={c()} variant="panel" tuning={!!pending()}>
                <div class="nowp__actions">
                  <button type="button" class="btn btn--primary btn--sm" onClick={enterFull}><Icon name="fullscreen" /> <span>Full screen</span></button>
                  <button type="button" class="btn btn--sm" aria-pressed={c().favorite} onClick={() => void fav(c())}>
                    <Icon name="star" /> <span>{c().favorite ? "Favourite" : "Add to favourites"}</span>
                  </button>
                  <button type="button" class="btn btn--sm btn--quiet" aria-pressed={stats()} onClick={toggleStats}>
                    <Icon name="activity" /> <span>Zap timing</span>
                  </button>
                </div>
              </NowPlaying>
              <Later channel={c()} />
            </div>
          )}
        </Show>
      </section>

      <Show when={mini()}>
        <div class="miniguide" data-nav-trap role="dialog" aria-label="Channels">
          <header class="miniguide__head">
            <strong>{filter().kind === "all" ? "All channels" : filter().kind === "favorites" ? "Favourites" : (filter() as { name: string }).name}</strong>
            <span>OK to watch · Back to close</span>
          </header>
          <ChannelList
            class="miniguide__list"
            channels={list()}
            activeId={tuner.channelId()}
            onPick={(c) => {
              setMini(false);
              if (c.id !== tuner.channelId()) tuneNow(c);
              requestAnimationFrame(() => screen?.focus({ preventScroll: true }));
            }}
            controller={(a) => (miniApi = a)}
            rowRem={4.4}
            label="Mini guide"
          />
        </div>
        <div class="miniguide__scrim" onClick={closeMini} aria-hidden="true" />
      </Show>
    </div>
  );
}

/** The next few programmes on the tuned channel. */
function Later(props: { channel: IptvChannel }): JSX.Element {
  const [data] = createResource(
    () => props.channel.id,
    async (id) => {
      const from = nowSec();
      const g = await iptv.epg({ from, to: from + 8 * 3600, channels: [id] });
      return (g.channels[0]?.programs ?? []).filter((p) => p.start > from).slice(1, 5);
    },
  );
  return (
    <Show when={data()?.length}>
      <div class="later">
        <p class="eyebrow">Later on {props.channel.name}</p>
        <ol class="later__list">
          <For each={data()}>
            {(p) => (
              <li>
                <span class="later__time">{fmtRange(p)}</span>
                <span class="later__title">{p.title}</span>
              </li>
            )}
          </For>
        </ol>
      </div>
    </Show>
  );
}

function LiveSkeleton(): JSX.Element {
  return (
    <div class="live is-skeleton" aria-busy="true" aria-label="Loading channels">
      <aside class="live__side">
        <header class="live__head"><div><span class="skel skel--text" style={{ width: "6rem" }} /><span class="skel skel--title" style={{ width: "12rem", height: "2.4rem", "margin-top": "0.6rem" }} /></div></header>
        <div class="chlist">
          {Array.from({ length: 9 }, () => (
            <div class="chrow chrow--skel"><span class="skel" style={{ width: "100%", height: "100%" }} /></div>
          ))}
        </div>
      </aside>
      <section class="live__stage"><div class="live__screen skel" /></section>
    </div>
  );
}

function LiveEmpty(): JSX.Element {
  const [sources] = createResource(() => (isAdmin() ? iptv.sources() : Promise.resolve(null)));
  onMount(() => requestAnimationFrame(() => {
    const el = document.querySelector<HTMLElement>(".live-empty [data-autofocus]");
    if (el) focusElement(el, { smooth: false });
  }));
  return (
    <div class="live-empty">
      <div class="live-empty__art" aria-hidden="true">
        <div class="live-empty__tv">
          <i /><i /><i /><i /><i /><i /><i />
        </div>
      </div>
      <div class="live-empty__copy">
        <p class="eyebrow eyebrow--accent">Live TV</p>
        <Show
          when={isAdmin()}
          fallback={
            <>
              <h1 class="state__title">Live TV isn't set up yet</h1>
              <p class="state__body">Once the person who runs this server adds a TV provider, channels and the guide will show up here.</p>
            </>
          }
        >
          <Show
            when={sources() && sources()!.length > 0}
            fallback={
              <>
                <h1 class="state__title">Bring your TV channels into Reel</h1>
                <p class="state__body">
                  Add an IPTV provider — an M3U playlist or an Xtream Codes login — and Reel builds the channel list and guide, keeps one connection per
                  channel and shares it between everyone watching.
                </p>
                <div class="actions">
                  <a class="btn btn--primary btn--lg" href={href("/admin/iptv")} data-autofocus><Icon name="plus" /> <span>Add a TV provider</span></a>
                </div>
              </>
            }
          >
            <h1 class="state__title">Your channels are on their way</h1>
            <p class="state__body">
              {sources()!.some((s) => s.refreshing)
                ? `Reel is loading channels from ${sources()!.find((s) => s.refreshing)!.name}. Big playlists can take a minute.`
                : `${sources()!.length === 1 ? sources()![0]!.name : "Your providers"} didn't return any enabled channels yet. Check the source in settings.`}
            </p>
            <div class="actions">
              <button type="button" class="btn btn--primary btn--lg" data-autofocus onClick={() => void loadChannels(true)}><Icon name="retry" /> <span>Check again</span></button>
              <a class="btn btn--lg" href={href("/admin/iptv")}><Icon name="gear" /> <span>IPTV settings</span></a>
            </div>
          </Show>
        </Show>
      </div>
    </div>
  );
}

function readFlag(k: string): boolean {
  try {
    return localStorage.getItem(k) === "1";
  } catch {
    return false;
  }
}
function writeFlag(k: string, v: boolean): void {
  try {
    if (v) localStorage.setItem(k, "1");
    else localStorage.removeItem(k);
  } catch {
    /* ignore */
  }
}
