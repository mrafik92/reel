import { createEffect, createMemo, createResource, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { useNavigate, useSearchParams } from "@solidjs/router";
import { focusElement, Icon, onBack, onRemoteKey, SheetClose } from "@reel/ui";
import "../styles/live.css";
import { iptv, type IptvChannel } from "../api/iptv";
import { href } from "../lib/mode";
import { isAdmin } from "../lib/auth";
import { toast } from "../lib/toast";
import { ErrorState } from "../components/States";
import { ChannelList, type ChannelListApi } from "../components/live/ChannelList";
import { ChannelLogo } from "../components/live/ChannelLogo";
import { NowPlaying, NumberEntry, ZapStats } from "../components/live/Overlays";
import { ChannelSearch, GroupPicker } from "../components/live/Filters";
import { createTuner, forgetSession, ownSession, type TuneError } from "../components/live/tuner";
import {
  allChannels, channelById, channelByNumber, channelsError, current, favourites, filter, filtered, filterLabel, fmtRange, fmtTime,
  lastChannel, loadAll, loadChannels, loadState, query, rememberChannel, setQuery, toggleFavorite, nowSec, clock, ensureNow, filterKey,
  totalChannels, noteWatched, nowNext, progressOf, getBrowsingChannel, setBrowsingChannel, setFilter,
} from "../components/live/store";
import { currentUser } from "../lib/auth";
import { createTapToggle } from "../lib/tap";

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
        <Match when={loadState() === "error"}>
          <div class="page"><ErrorState error={channelsError()} onRetry={() => void loadChannels(true)} title="Couldn't load your channels" /></div>
        </Match>
        <Match when={loadState() === "loading"}>
          <LiveSkeleton />
        </Match>
        <Match when={loadState() === "empty"}>
          <LiveEmpty />
        </Match>
        <Match when={loadState() === "ready"}>
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
  const [tunedCh, setTunedCh] = createSignal<IptvChannel>();
  const [numWait, setNumWait] = createSignal(false);
  const [focusedChannel, setFocusedChannel] = createSignal<number | undefined>(getBrowsingChannel());

  const tuned = () => channelById(tuner.channelId()) ?? tunedCh();
  /**
   * What the hero shows before anything is tuned. Opening Live TV never tunes by itself — on a
   * one-connection plan that would grab the slot — so it offers the last channel instead.
   */
  const heroCh = () => tuned() ?? lastChannel() ?? list()[0];
  const idle = () => tuner.phase() === "idle" && !pending();
  /** What the overlays describe: the zap target while it's pending, else the tuned channel. */
  const shown = () => pending() ?? tuned();
  // Tizen's AVPlay draws on a full-screen plane behind the page: no windowed preview there.
  const isFull = () => full() || (tuner.nativePlane() && tuner.phase() !== "idle");
  /** What's in the side list: search results or the current group / favourites. */
  const shownList = () => current();
  /** What Up/Down zaps through: the visible list if the tuned channel is in it, else the first list that has it. */
  const list = createMemo(() => {
    const id = tuner.channelId();
    const cands = [current(), filtered(), favourites(), allChannels()];
    if (id != null) for (const l of cands) if (l && l.some((c) => c.id === id)) return l;
    for (const l of cands) if (l && l.length) return l;
    return [] as IptvChannel[];
  });
  const maxDigits = createMemo(() => {
    const a = allChannels();
    let m = 0;
    if (a) a.forEach((c) => c.number != null && c.number > m && (m = c.number));
    else m = totalChannels();
    return Math.min(5, Math.max(1, String(m).length));
  });

  // Overlays need now/next even for channels that aren't on screen in the list.
  createEffect(() => {
    const c = shown() ?? heroCh();
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
    rememberChannel(c);
    noteWatched(c.group);
    setTunedCh(c);
    setPending(undefined);
    showOsd();
    void tuner.tune(c);
  };

  let zapTimer: ReturnType<typeof setTimeout> | undefined;
  const zap = (dir: 1 | -1) => {
    const l = list();
    if (!l.length) return;
    const base = pending() ?? tuned() ?? heroCh();
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
    else {
      tuneNow(c);
      enterFull();
    }
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
    if (!c && !allChannels()) {
      // Numbers need the full list, which may still be loading.
      setNumWait(true);
      void loadAll()
        .then(() => {
          setNumWait(false);
          if (digits() === String(n)) commitDigits();
        })
        .catch(() => {
          setNumWait(false);
          setMissing(true);
          digitTimer = setTimeout(clearDigits, 1400);
        });
      return;
    }
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
    const id = focusedChannel() ?? tuner.channelId();
    requestAnimationFrame(() => id != null && listApi?.reveal(id, true));
  };
  // Where focus goes back to when the mini guide closes: the "Channel list" button when it
  // was opened from there, else the screen (it was opened with ←/OK on the picture).
  let miniOpener: HTMLElement | null = null;
  const openMini = () => {
    const a = document.activeElement;
    miniOpener = a instanceof HTMLButtonElement && a.closest(".live__top") ? a : null;
    setMini(true);
    setOsd(false);
    requestAnimationFrame(() => {
      const id = (pending() ?? tuned())?.id;
      if (id != null) miniApi?.reveal(id, true);
    });
  };
  const closeMini = () => {
    setMini(false);
    const back = miniOpener?.isConnected ? miniOpener : null;
    miniOpener = null;
    if (back) showOsd();
    requestAnimationFrame(() => (back ? focusElement(back, { smooth: false }) : screen?.focus({ preventScroll: true })));
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
    if (started) return;
    const want = Number(params.ch) || undefined;
    started = true;
    // Only an explicit request (Watch in the guide) tunes on arrival.
    const c = want ? channelById(want) : undefined;
    if (!c) return;
    tuneNow(c);
    setParams({ ch: undefined }, { replace: true });
    enterFull();
  });
  // Land the remote on the tuned channel's card (or the first one) once the row has rendered.
  let landed = false;
  createEffect(() => {
    const l = shownList();
    if (landed || !l || !l.length) return;
    landed = true;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const a = document.activeElement;
        if (isFull() || (a && a !== document.body && a.tagName !== "MAIN")) return;
        const id = focusedChannel() ?? tuner.channelId() ?? tunedCh()?.id;
        listApi?.reveal(id != null && l.some((c) => c.id === id) ? id : l[0]!.id, true);
      }),
    );
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
      if (document.querySelector("[data-nav-trap]") && !mini()) return false;
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
          // With an error card up, OK goes to its buttons.
          if (tuner.error() || document.activeElement?.closest("button, a")) return false;
          openMini();
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
      if (query()) {
        setQuery("");
        return true;
      }
      return false;
    });
    // Desktop shortcuts.
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
      if (t?.closest("[data-nav-trap]")) return;
      if (e.key === "f" || e.key === "F") (isFull() ? exitFull() : enterFull());
      else if (e.key === "i" || e.key === "I") toggleStats();
      else if (e.key === "g" || e.key === "G") navigate("/live/guide");
      else if (e.key === "m" || e.key === "M") void fav();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    // Give the provider slot back when nobody's looking: page hidden (pagehide = right away,
    // tab in the background = after 30 s). Coming back re-tunes the same channel.
    let hiddenTimer: ReturnType<typeof setTimeout> | undefined;
    let parked: IptvChannel | undefined;
    const park = () => {
      if (tuner.phase() === "idle" || tuner.phase() === "error") return;
      parked = tuned();
      tuner.stop(true);
    };
    const unpark = () => {
      if (hiddenTimer) clearTimeout(hiddenTimer);
      hiddenTimer = undefined;
      const c = parked;
      parked = undefined;
      if (c && tuner.phase() === "idle") tuneNow(c);
    };
    const onHide = () => park();
    const onShow = (e: PageTransitionEvent) => e.persisted && unpark();
    const onVis = () => {
      if (document.hidden) {
        if (hiddenTimer) clearTimeout(hiddenTimer);
        hiddenTimer = setTimeout(park, 30_000);
      } else unpark();
    };
    window.addEventListener("pagehide", onHide);
    window.addEventListener("pageshow", onShow);
    document.addEventListener("visibilitychange", onVis);
    onCleanup(() => {
      offKeys();
      offBack();
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pagehide", onHide);
      window.removeEventListener("pageshow", onShow);
      document.removeEventListener("visibilitychange", onVis);
      if (hiddenTimer) clearTimeout(hiddenTimer);
      [osdTimer, zapTimer, digitTimer].forEach((t) => t && clearTimeout(t));
      tuner.dispose();
    });
  });

  // Full screen: a click or tap shows/hides the overlay. Live TV can't pause, so unlike the
  // library player (#183) a mouse click doesn't pause here either. In the page: a click tunes the
  // idle hero channel or goes full screen. Single clicks wait out a possible double-click so it
  // doesn't flicker the overlay (see lib/tap).
  const activate = () => {
    const h = heroCh();
    if (idle() && h) tuneNow(h);
    else enterFull();
  };
  const tap = createTapToggle({
    shown: () => isFull() && osd(),
    show: () => (isFull() ? showOsd() : activate()),
    hide: () => (isFull() ? setOsd(false) : activate()),
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
    <div class="live live--rows" classList={{ "is-full": isFull(), "is-native": tuner.nativePlane(), "has-mini": mini() }}>
      <section class="live__hero" inert={!isFull()} aria-hidden={!isFull()}>
        <div
          ref={screen}
          class="live__screen"
          classList={{ "is-osd": osd() || !!pending(), "is-error": !!tuner.error(), "is-limit": tuner.error()?.kind === "limit" }}
          tabindex={-1}
          aria-label={isFull() ? `Watching ${tuned()?.name ?? ""}` : "Watch full screen"}
          onPointerDown={tap.onPointerDown}
          onClick={tap.onClick}
          onDblClick={() => {
            tap.cancel();
            if (isFull()) exitFull();
            else enterFull();
          }}
          onMouseMove={onPointer}
        >
          <video ref={video} class="live__video" playsinline autoplay />
          <Show when={!isFull() && idle() && heroCh()}>
            {(c) => <IdleHero channel={c()} onWatch={() => tuneNow(c())} />}
          </Show>
          <div class="live__vignette" aria-hidden="true" />

          <Show when={tuner.phase() === "tuning" && !tuner.error()}>
            <div class="live__tuning" aria-live="polite">
              <span class="spinner" />
              <span>{tuner.recovering() ?? `Tuning ${tuned()?.name ?? ""}`}…</span>
            </div>
          </Show>
          <Show when={tuner.phase() === "playing" && (tuner.buffering() || tuner.recovering())}>
            <div class="live__tuning" aria-live="polite">
              <span class="spinner" />
              <Show when={tuner.recovering()}>{(r) => <span>{r()}…</span>}</Show>
            </div>
          </Show>

          <Show when={tuner.error()}>
            {(e) => (
              <div
                class="live__error"
                role={e().kind === "limit" ? "status" : "alert"}
                classList={{ [`is-${e().kind}`]: true, "is-calm": e().kind === "limit" }}
                onClick={(ev) => ev.stopPropagation()}
              >
                <Icon name={e().kind === "limit" ? "tv" : "alert"} />
                <h2>{e().title}</h2>
                <p>{e().body}</p>
                <div class="actions actions--center">
                  <Show when={e().provider}>
                    <StopOther error={e()} onStopped={() => setTimeout(() => tuner.retry(), 700)} onTakeover={() => tuner.retry({ takeover: true })} />
                  </Show>
                  <Show when={e().retry}>
                    <button type="button" class="btn btn--sm" classList={{ "btn--primary": !e().provider }} onClick={() => tuner.retry()}><Icon name="retry" /> <span>Try again</span></button>
                  </Show>
                </div>
                <Show when={isFull()}><p class="live__hint">▲▼ another channel · Back for the list</p></Show>
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
              <button type="button" class="iconbtn" aria-label={tuned()?.favorite ? "Remove from favourites" : "Add to favourites"} aria-pressed={!!tuned()?.favorite} onClick={() => void fav()}><Icon name="star" /></button>
              <button type="button" class="iconbtn" aria-label="Zap timing" aria-pressed={stats()} onClick={toggleStats}><Icon name="activity" /></button>
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

        <div class="live__info">
          <div class="live__infohead">
            <p class="eyebrow eyebrow--accent">Live TV</p>
            <a class="btn btn--sm" href={href("/live/guide")}><Icon name="grid" /> <span>Guide</span></a>
          </div>
          <Show when={!isFull() && (shown() ?? heroCh())} fallback={<p class="nowp__none">Pick a channel below to start watching.</p>}>
            {(c) => (
              <NowPlaying channel={c()} variant="panel" tuning={!!pending()} idle={idle()}>
                <div class="nowp__actions">
                  <Show
                    when={!idle()}
                    fallback={<button type="button" class="btn btn--primary btn--sm" onClick={() => tuneNow(c())}><Icon name="play" /> <span>Watch</span></button>}
                  >
                    <button type="button" class="btn btn--primary btn--sm" onClick={enterFull}><Icon name="fullscreen" /> <span>Full screen</span></button>
                  </Show>
                  <button type="button" class="btn btn--sm" aria-pressed={c().favorite} onClick={() => void fav(c())}>
                    <Icon name="star" /> <span>{c().favorite ? "Favourite" : "Favourite it"}</span>
                  </button>
                  <button type="button" class="btn btn--sm btn--quiet" aria-pressed={stats()} onClick={toggleStats}>
                    <Icon name="activity" /> <span>Zap timing</span>
                  </button>
                </div>
              </NowPlaying>
            )}
          </Show>
        </div>
      </section>

      <section class="live__rows" aria-label="Channels">
        <div class="live__browser-head">
          <h1>Live TV</h1>
          <a class="btn btn--sm" href={href("/live/guide")} onClick={() => { const id = focusedChannel(); if (id != null && !filtered()?.some((c) => c.id === id)) setFilter({ kind: "all" }); }}><Icon name="grid" /> <span>Guide</span></a>
        </div>
        <div class="live__browse-controls"><GroupPicker /><ChannelSearch class="livesearch--row" /></div>
        <div class="chanrow">
          <div class="rowlabel rowlabel--split">
            <p class="rowlabel__title">
              <span dir="auto">{query() ? "Search results" : filterLabel()}</span>{" "}
              <small>{shownList() ? shownList()!.length.toLocaleString() : "…"}</small>
            </p>
          </div>
          <Show when={shownList()} fallback={<RailLoading />}>
            <ChannelList
              channels={shownList()!}
              resetKey={`${filterKey(filter())}|${query()}`}
              activeId={tuner.phase() === "error" ? undefined : tuner.channelId()}
              pendingId={pending()?.id ?? digitTarget()?.id}
              onPick={pick}
              onFocusChannel={(c) => { setFocusedChannel(c.id); setBrowsingChannel(c.id); }}
              controller={(a) => (listApi = a)}
              empty={
                <>
                  <Icon name={query() ? "search" : "star"} />
                  <p>
                    {query()
                      ? `No channels match “${query()}”.`
                      : filter().kind === "favorites"
                        ? "No favourites yet. Pick a group, then press the yellow button (or M) while watching to add one."
                        : "No channels in this group."}
                  </p>
                </>
              }
            />
          </Show>
        </div>
        <Show when={focusedChannel() != null && shownList()?.some((c) => c.id === focusedChannel()) && nowNext(focusedChannel()!)?.next}>
          {(next) => <div class="live__next"><span>Next on {channelById(focusedChannel())?.name}</span><strong>{fmtTime(next().start)} · {next().title}</strong></div>}
        </Show>
      </section>

      <Show when={mini()}>
        <div class="miniguide" data-nav-trap role="dialog" aria-label="Channels">
          <header class="miniguide__head">
            <strong>{filter().kind === "all" ? "All channels" : filter().kind === "favorites" ? "Favourites" : (filter() as { name: string }).name}</strong>
            <span>OK to watch · Back to close</span>
            <SheetClose class="miniguide__close" onClose={closeMini} />
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

function LiveSkeleton(): JSX.Element {
  return (
    <div class="live live--rows is-skeleton" aria-busy="true" aria-label="Loading channels">
      <section class="live__hero">
        <div class="live__screen skel" />
        <div class="live__info">
          <span class="skel skel--text" style={{ width: "6rem" }} />
          <span class="skel skel--title" style={{ width: "70%", height: "2.4rem", "margin-top": "1.4rem" }} />
          <span class="skel skel--text" style={{ width: "50%", "margin-top": "1rem" }} />
        </div>
      </section>
      <section class="live__rows">
        <div class="grouprow"><p class="rowlabel"><span>Groups</span></p><div class="livefilters"><span class="skel" style={{ width: "60%", height: "2.35rem" }} /></div></div>
        <div class="chanrow"><div class="rowlabel rowlabel--split"><span class="skel skel--text" style={{ width: "10rem" }} /></div><RailLoading /></div>
      </section>
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

function RailLoading(): JSX.Element {
  return (
    <div class="chrail" aria-busy="true" aria-label="Loading channels">
      <div class="chrail__track">
        {Array.from({ length: 8 }, (_v, i) => (
          <div class="chcard chcard--skel" style={{ left: `${i * 15.5}rem`, width: "14.5rem" }}><span class="skel" /></div>
        ))}
      </div>
    </div>
  );
}

/**
 * "Stop the other stream":
 * - admins on the new contract (409 + holders): take the connection over (`takeover: true`);
 * - older servers (429 "in use by: …"): admins stop the matching live session from the hub;
 * - everyone: a stream this browser left running (e.g. after a reload).
 */
function StopOther(props: { error: TuneError; onStopped: () => void; onTakeover: () => void }): JSX.Element {
  const [busy, setBusy] = createSignal(false);
  const [target] = createResource(
    () => props.error,
    async (err) => {
      const me = currentUser()?.id;
      if (err.holders?.length) {
        const h = err.holders[0]!;
        const mine = err.holders.every((x) => x.user_id === me);
        return { id: "", channel: err.holders.map((x) => x.channel_name).join(", "), mine, who: h.username ?? "another viewer", takeover: true };
      }
      const names = err.watching ?? [];
      if (isAdmin()) {
        try {
          const list = await iptv.liveSessions();
          const hits = list.filter((s) => !names.length || names.indexOf(s.channel_name) >= 0);
          const mine = hits.find((s) => s.user_id === me);
          const pick = mine ?? hits[0];
          if (pick) return { id: pick.id, channel: pick.channel_name, mine: !!mine, who: pick.username ?? `user ${pick.user_id}`, takeover: false };
        } catch {
          /* fall through */
        }
      }
      const own = ownSession();
      if (own && (!names.length || names.indexOf(own.channel) >= 0)) return { id: own.id, channel: own.channel, mine: true, who: "", takeover: false };
      return undefined;
    },
  );
  const stop = async () => {
    const t = target();
    if (!t || busy()) return;
    if (t.takeover) return props.onTakeover();
    setBusy(true);
    try {
      await iptv.stop(t.id);
      forgetSession(t.id);
      toast(`Stopped ${t.channel}`, "success", 2000);
      props.onStopped();
    } catch (e) {
      // Already gone: the slot frees up on its own.
      if (e instanceof Error && /not found/i.test(e.message)) props.onStopped();
      else toast(e instanceof Error ? e.message : "Couldn't stop that stream", "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={target()}>
      {(t) => (
        <button type="button" class="btn btn--primary btn--sm" data-autofocus aria-disabled={busy() ? "true" : undefined} onClick={() => void stop()}>
          <Icon name="x" />
          <span>
            {busy()
              ? "Stopping…"
              : t().mine
                ? `Stop the other stream (${t().channel})`
                : t().takeover
                  ? "Take over"
                  : `Stop ${t().who}'s stream (${t().channel})`}
          </span>
        </button>
      )}
    </Show>
  );
}

/** Before anything plays: the last channel as a still "poster" with an explicit Watch button. */
function IdleHero(props: { channel: IptvChannel; onWatch: () => void }): JSX.Element {
  const nn = () => nowNext(props.channel.id);
  return (
    <div class="live__idle" onClick={(e) => e.stopPropagation()}>
      <div class="live__idle-glow" aria-hidden="true" />
      <ChannelLogo channel={props.channel} class="chlogo--hero" />
      <p class="eyebrow">{lastChannel()?.id === props.channel.id ? "Last watched" : "Ready to watch"}</p>
      <h2 class="live__idle-name" dir="auto">
        <Show when={props.channel.number != null}><span class="live__idle-num">{props.channel.number}</span> </Show>
        {props.channel.name}
      </h2>
      <Show when={nn()?.now}>
        {(p) => (
          <p class="live__idle-now">
            <span>{p().title}</span>
            <span class="live__idle-bar" aria-hidden="true"><i style={{ width: `${progressOf(p(), clock()) * 100}%` }} /></span>
          </p>
        )}
      </Show>
      <button type="button" class="btn btn--primary btn--lg live__watch" onClick={props.onWatch}>
        <Icon name="play" /> <span>Watch {props.channel.name}</span>
      </button>
      <p class="live__idle-hint">Nothing plays until you choose — your TV plan has a limited number of connections.</p>
    </div>
  );
}
