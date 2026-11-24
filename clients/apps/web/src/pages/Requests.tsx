import { createMemo, createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { formatEta, formatSpeed, Icon, Img, ProgressRing, RING_LABEL } from "@reel/ui";
import { api, invalidate, type RequestDto, type RequestState } from "../api";
import { downloads, noteRequest } from "../lib/downloads";
import { isAdmin } from "../lib/auth";
import { useRequestActions } from "../lib/requests";
import { usePageFocus } from "../lib/focus";
import { toast } from "../lib/toast";
import { href } from "../lib/mode";
import { EmptyState, ErrorState } from "../components/States";
import { useDiscoverSheet } from "../components/Discover";

type Tab = "all" | "pending" | "active" | "available" | "problems";
const TABS: { id: Tab; label: string }[] = [
  { id: "all", label: "All" },
  { id: "active", label: "In progress" },
  { id: "pending", label: "Awaiting approval" },
  { id: "available", label: "Available" },
  { id: "problems", label: "Problems" },
];
const ACTIVE: RequestState[] = ["approved", "searching", "queued", "downloading", "stalled", "importing"];

function relTime(unix: number): string {
  const d = Date.now() / 1000 - unix;
  if (d < 90) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  if (d < 86400 * 14) {
    const n = Math.round(d / 86400);
    return n === 1 ? "yesterday" : `${n} days ago`;
  }
  return new Date(unix * 1000).toLocaleDateString();
}

export default function Requests(): JSX.Element {
  const [list, { refetch, mutate }] = createResource(() => api.requests({ limit: 300 }));
  const [users] = createResource(() => (isAdmin() ? api.users().catch(() => []) : Promise.resolve([])));
  const [tab, setTab] = createSignal<Tab>("all");
  const actions = useRequestActions();
  const sheet = useDiscoverSheet();
  const nameOf = (id: number) => users()?.find((u) => u.id === id)?.username;

  onMount(() => {
    const t = setInterval(() => {
      invalidate("requests");
      void refetch();
    }, 30_000);
    onCleanup(() => clearInterval(t));
  });

  /** Live state wins over the list snapshot. */
  const stateOf = (r: RequestDto): RequestState => {
    const live = downloads.get(r.tmdb_id);
    return live && String(live.request_id) === String(r.id) ? live.state : r.status;
  };
  const filtered = createMemo(() => {
    const all = list() ?? [];
    switch (tab()) {
      case "pending": return all.filter((r) => stateOf(r) === "requested");
      case "active": return all.filter((r) => ACTIVE.indexOf(stateOf(r)) >= 0);
      case "available": return all.filter((r) => stateOf(r) === "available");
      case "problems": return all.filter((r) => /failed|not_found|stalled|declined/.test(stateOf(r)));
      default: return all;
    }
  });
  const count = (t: Tab) => {
    const prev = tab();
    if (t === prev) return filtered().length;
    const all = list() ?? [];
    if (t === "all") return all.length;
    if (t === "pending") return all.filter((r) => stateOf(r) === "requested").length;
    if (t === "active") return all.filter((r) => ACTIVE.indexOf(stateOf(r)) >= 0).length;
    if (t === "available") return all.filter((r) => stateOf(r) === "available").length;
    return all.filter((r) => /failed|not_found|stalled|declined/.test(stateOf(r))).length;
  };

  const replace = (r: RequestDto) => mutate((l) => (l ?? []).map((x) => (x.id === r.id ? r : x)));
  const approve = async (r: RequestDto) => {
    try {
      const n = await api.approveRequest(r.id);
      replace(n);
      noteRequest(n);
      toast(`Approved “${r.title ?? "request"}”`, "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't approve", "error");
    }
  };
  const decline = async (r: RequestDto) => {
    try {
      replace(await api.declineRequest(r.id));
      toast(`Declined “${r.title ?? "request"}”`);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't decline", "error");
    }
  };

  let el!: HTMLDivElement;
  usePageFocus(() => !!list.error || !!list(), () => el);

  return (
    <div class="page requests" ref={el}>
      <header class="page-head">
        <p class="eyebrow eyebrow--accent">{isAdmin() ? "Everyone's requests" : "Your requests"}</p>
        <h1 class="page-title">Requests</h1>
      </header>

      <nav class="tabs" aria-label="Filter" data-nav-group>
        <For each={TABS}>
          {(t) => (
            <button type="button" class="tab" aria-current={tab() === t.id ? "true" : undefined} data-nav-default={tab() === t.id ? "" : undefined} onClick={() => setTab(t.id)}>
              {t.label}
              <Show when={list() && count(t.id)}><span class="tab__count">{count(t.id)}</span></Show>
            </button>
          )}
        </For>
      </nav>

      <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={refetch} />}>
        <Show when={list()} fallback={<div class="reqlist is-loading" aria-busy="true" />}>
          <Show
            when={filtered().length}
            fallback={
              <EmptyState
                title={tab() === "all" ? "No requests yet" : "Nothing here"}
                body={tab() === "all" ? "Find something in Discover and press Add — it'll show up here while it downloads." : undefined}
              >
                <Show when={tab() === "all"}>
                  <a class="btn btn--primary" href={href("/discover")}><Icon name="compass" /> <span>Go to Discover</span></a>
                </Show>
              </EmptyState>
            }
          >
            <ul class="reqlist">
              <For each={filtered()}>
                {(r) => {
                  const live = () => {
                    const d = downloads.get(r.tmdb_id);
                    return d && String(d.request_id) === String(r.id) ? d : undefined;
                  };
                  const st = () => stateOf(r);
                  const prog = () => live()?.progress ?? (st() === "available" ? 1 : 0);
                  return (
                    <li class="req" data-fid={`r:${r.id}`}>
                      <button
                        type="button"
                        class="req__main"
                        aria-label={`${r.title ?? "Request"}: ${RING_LABEL[st()]}`}
                        onClick={() => sheet.open({ tmdb_id: r.tmdb_id, media_type: r.media_type, title: r.title ?? "", poster_url: r.poster_url, status: "requested", progress: null })}
                      >
                      <Img class="req__poster" src={r.poster_url} width={185} fallback={<div class="card__fallback"><span>{r.title}</span></div>} />
                      <div class="req__info">
                        <strong class="req__title">{r.title ?? `TMDB #${r.tmdb_id}`}</strong>
                        <span class="req__meta">
                          {r.media_type === "tv" ? "Series" : "Film"}
                          {r.media_type === "tv" && r.seasons && r.seasons.length ? ` · Season${r.seasons.length > 1 ? "s" : ""} ${r.seasons.join(", ")}` : ""}
                          {" · "}
                          {relTime(r.created_at)}
                          <Show when={isAdmin()}> · {nameOf(r.user_id) ?? (r as RequestDto & { username?: string }).username ?? `user ${r.user_id}`}</Show>
                        </span>
                        <Show when={r.note}><span class="req__note">“{r.note}”</span></Show>
                      </div>
                      </button>
                      <div class={`req__status req__status--${st()}`}>
                        <Show when={st() !== "declined"} fallback={<Icon name="x" class="req__x" />}>
                          <ProgressRing state={st()} progress={prog()} size="2.8rem" />
                        </Show>
                        <div>
                          <strong>{RING_LABEL[st()]}</strong>
                          <span>
                            {st() === "downloading" && live()
                              ? [`${Math.floor(prog() * 100)}%`, live()!.eta_seconds ? formatEta(live()!.eta_seconds!) : "", live()!.speed_bps ? formatSpeed(live()!.speed_bps!) : ""].filter(Boolean).join(" · ")
                              : st() === "requested" ? "Waiting for approval" : st() === "stalled" ? "No progress — still trying" : ""}
                          </span>
                        </div>
                      </div>
                      <div class="req__actions">
                        <Show when={isAdmin() && st() === "requested"}>
                          <button type="button" class="btn btn--accent btn--sm" onClick={() => void approve(r)}><Icon name="check" /> <span>Approve</span></button>
                          <button type="button" class="btn btn--sm" onClick={() => void decline(r)}><Icon name="x" /> <span>Decline</span></button>
                        </Show>
                        <Show when={st() === "failed" || st() === "not_found"}>
                          <button type="button" class="btn btn--accent btn--sm" onClick={() => void actions.retry(r.id, r.title ?? undefined)}><Icon name="retry" /> <span>Retry</span></button>
                        </Show>
                        <Show when={st() === "available"}>
                          <button type="button" class="btn btn--primary btn--sm" onClick={() => void actions.play({ title: r.title ?? "", media_type: r.media_type, tmdb_id: r.tmdb_id })}>
                            <Icon name="play" /> <span>Play</span>
                          </button>
                        </Show>
                      </div>
                    </li>
                  );
                }}
              </For>
            </ul>
          </Show>
        </Show>
      </Show>
      {sheet.view()}
    </div>
  );
}
