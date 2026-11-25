import type { JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { href } from "../lib/mode";
import { usePageFocus } from "../lib/focus";

export default function NotFound(): JSX.Element {
  usePageFocus(() => true);
  return (
    <div class="page">
      <div class="state">
        <p class="eyebrow eyebrow--accent">404</p>
        <h1 class="state__title">This page isn’t showing</h1>
        <p class="state__body">The link may be old, or the title was removed from your library.</p>
        <a class="btn btn--primary" href={href("/")} data-autofocus>
          <Icon name="home" /> <span>Back to Home</span>
        </a>
      </div>
    </div>
  );
}
