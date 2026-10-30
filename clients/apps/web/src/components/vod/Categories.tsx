import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, Icon, onBack } from "@reel/ui";
import type { VodCategory, VodId } from "../../api/vod";

/** "AR | Egyptian Movies" → { tag: "AR", name: "Egyptian Movies" }; plain names pass through. */
export function splitCategory(name: string): { tag?: string; name: string } {
  const m = /^\s*([^|:]{1,16}?)\s*[|:]\s*(.+)$/.exec(name);
  if (m && m[1] && m[2] && m[1].length <= 14) return { tag: m[1].trim(), name: m[2].trim() };
  return { name: name.trim() };
}

const fold = (s: string) => s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");

/** Category label: provider prefix as a small tag, then the name. */
export function CategoryLabel(props: { name: string }): JSX.Element {
  const p = () => splitCategory(props.name);
  return (
    <>
      <Show when={p().tag}><span class="ctag">{p().tag}</span></Show>
      <span class="cname" dir="auto">{p().name}</span>
    </>
  );
}

/**
 * Every provider bucket, searchable — there can be 100+. Grouped by the provider's prefix
 * (AR, EN, NETFLIX…) in the provider's own order.
 */
export function CategorySheet(props: {
  categories: VodCategory[];
  active?: VodId | null;
  noun: string;
  onPick: (id: VodId | null) => void;
  onClose: () => void;
}): JSX.Element {
  const [q, setQ] = createSignal("");
  let panel!: HTMLDivElement;
  let input!: HTMLInputElement;
  let returnTo: HTMLElement | null = null;
  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => {
      // Remote: land on the current bucket. Mouse/keyboard users get the search box.
      const cur = panel.querySelector<HTMLElement>('[aria-current="true"]');
      const pointer = document.documentElement.getAttribute("data-input") === "pointer";
      if (cur && !pointer) focusElement(cur, { smooth: false });
      else focusElement(input, { smooth: false });
    });
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(() => {
      off();
      if (returnTo && returnTo.isConnected) focusElement(returnTo, { smooth: false });
    });
  });

  const groups = createMemo(() => {
    const n = fold(q().trim());
    const out: { tag: string; items: VodCategory[] }[] = [];
    const idx = new Map<string, number>();
    props.categories.forEach((c) => {
      if (n && fold(c.name).indexOf(n) < 0) return;
      const tag = splitCategory(c.name).tag ?? "Other";
      let i = idx.get(tag);
      if (i == null) {
        i = out.length;
        idx.set(tag, i);
        out.push({ tag, items: [] });
      }
      out[i]!.items.push(c);
    });
    return out;
  });
  const shown = () => groups().reduce((a, g) => a + g.items.length, 0);
  const pick = (id: VodId | null) => {
    props.onPick(id);
    props.onClose();
  };

  return (
    <Portal>
      <div class="sheet csheet" data-nav-trap data-nav-fixed>
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div
          class="sheet__panel csheet__panel"
          ref={panel}
          role="dialog"
          aria-modal="true"
          aria-label="Categories"
          onKeyDown={(e) => {
            // Start typing anywhere in the list: it goes to the search box.
            if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && document.activeElement !== input && /\S/.test(e.key)) {
              focusElement(input, { smooth: false });
            }
          }}
        >
          <header class="csheet__head">
            <p class="eyebrow eyebrow--accent">From your provider</p>
            <h2>Categories <span class="csheet__count">{props.categories.length}</span></h2>
            <label class="csheet__field">
              <Icon name="search" />
              <input
                ref={input}
                type="search"
                placeholder="Find a category"
                value={q()}
                onInput={(e) => setQ(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    const first = panel.querySelector<HTMLElement>(".csheet__list .sheet__opt");
                    if (first) focusElement(first, { smooth: false });
                  }
                }}
                autocomplete="off"
                spellcheck={false}
                aria-label="Find a category"
              />
            </label>
          </header>
          <div class="sheet__list csheet__list" data-nav-scroll="y">
            <Show when={!q().trim()}>
              <button type="button" class="sheet__opt" aria-current={props.active == null ? "true" : undefined} onClick={() => pick(null)}>
                <span class="sheet__mark">{props.active == null ? <Icon name="check" /> : null}</span>
                <span class="sheet__text"><span class="sheet__title">Everything</span><span class="sheet__detail">All {props.noun} on IPTV</span></span>
              </button>
            </Show>
            <For each={groups()}>
              {(g) => (
                <section class="csheet__group">
                  <h3 class="csheet__tag">{g.tag}</h3>
                  <For each={g.items}>
                    {(c) => {
                      const on = () => props.active != null && String(props.active) === String(c.id);
                      return (
                        <button type="button" class="sheet__opt csheet__opt" aria-current={on() ? "true" : undefined} onClick={() => pick(c.id)}>
                          <span class="sheet__mark">{on() ? <Icon name="check" /> : null}</span>
                          <span class="sheet__title csheet__name" dir="auto">{splitCategory(c.name).name}</span>
                          <span class="csheet__n">{c.count.toLocaleString()}</span>
                        </button>
                      );
                    }}
                  </For>
                </section>
              )}
            </For>
            <Show when={q().trim() && !shown()}>
              <p class="csheet__none">No category called “{q().trim()}”. Try a country code like <strong>AR</strong> or a service like <strong>Netflix</strong>.</p>
            </Show>
          </div>
          <footer class="sheet__foot">{shown().toLocaleString()} of {props.categories.length} categories</footer>
        </div>
      </div>
    </Portal>
  );
}

/** Scrollable chip rail of provider buckets, with "All" and a way into the full list. */
export function CategoryBar(props: {
  categories: VodCategory[];
  active?: VodId | null;
  browsing?: boolean;
  onPick: (id: VodId | null) => void;
  onMore: () => void;
}): JSX.Element {
  const isOn = (c: VodCategory) => props.active != null && String(props.active) === String(c.id);
  let scroller!: HTMLDivElement;
  // Keep the chosen bucket in view when it's deep in the rail.
  onMount(() =>
    requestAnimationFrame(() => {
      const el = scroller.querySelector<HTMLElement>('[aria-pressed="true"]');
      if (el && el.offsetLeft > scroller.clientWidth * 0.6) scroller.scrollLeft = el.offsetLeft - scroller.clientWidth * 0.3;
    }),
  );
  return (
    <div class="genre-bar cbar" data-nav-group>
      <div class="genre-bar__scroller" ref={scroller} data-nav-scroll="x" role="toolbar" aria-label="Categories">
        <div class="genre-bar__track">
          <button
            type="button"
            class="gchip gchip--all"
            aria-pressed={!props.browsing && props.active == null}
            data-nav-default={props.active == null ? "" : undefined}
            onClick={() => props.onPick(null)}
          >
            Everything
          </button>
          <button type="button" class="gchip cbar__more" onClick={() => props.onMore()} aria-label={`Find one of ${props.categories.length} categories`}>
            <Icon name="search" /> {props.categories.length} categories
          </button>
          <For each={props.categories}>
            {(c) => (
              <button
                type="button"
                class="gchip cchip"
                aria-pressed={isOn(c)}
                data-nav-default={isOn(c) ? "" : undefined}
                onClick={() => props.onPick(isOn(c) ? null : c.id)}
              >
                <CategoryLabel name={c.name} />
                <span class="gchip__count">{c.count.toLocaleString()}</span>
              </button>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}

