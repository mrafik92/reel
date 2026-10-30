import type { JSX } from "solid-js";

/** Stroke icons on a 24px grid. Kept inline so the TV bundle has no icon font. */
const PATHS = {
  home: "M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6h-6v6H4a1 1 0 0 1-1-1z",
  film: "M4 4h16v16H4zM8 4v16M16 4v16M4 8h4M4 12h4M4 16h4M16 8h4M16 12h4M16 16h4",
  tv: "M3 6h18v12H3zM8 21h8M12 18v3",
  search: "M10.5 18a7.5 7.5 0 1 0 0-15 7.5 7.5 0 0 0 0 15zM21 21l-5.2-5.2",
  inbox: "M3 13h5l1.5 3h5L16 13h5M5 5h14l2 8v6H3v-6z",
  live: "M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M5.6 5.6a9 9 0 0 0 0 12.8M18.4 5.6a9 9 0 0 1 0 12.8",
  play: "M7 4.5v15l12.5-7.5z",
  pause: "M7 4.5h3.5v15H7zM13.5 4.5H17v15h-3.5z",
  back: "M15 5l-7 7 7 7",
  forward: "M9 5l7 7-7 7",
  down: "M5 9l7 7 7-7",
  replay: "M4 12a8 8 0 1 0 2.4-5.7M4 4v4h4",
  skip: "M20 12a8 8 0 1 1-2.4-5.7M20 4v4h-4",
  restart: "M5 5v14M19 5 9 12l10 7z",
  cc: "M3 5h18v14H3zM10 10.5a2.5 2.5 0 1 0 0 3M17 10.5a2.5 2.5 0 1 0 0 3",
  audio: "M4 9v6h4l5 4V5L8 9zM16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12",
  check: "M4.5 12.5l5 5L19.5 7",
  alert: "M12 8v5M12 16.5v.5M10.3 3.9 2.6 17.5A2 2 0 0 0 4.3 20.5h15.4a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  retry: "M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7",
  x: "M6 6l12 12M18 6 6 18",
  plus: "M12 5v14M5 12h14",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7v5l3 2",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v6M12 7.5v.5",
  layers: "M12 3 2 8l10 5 10-5zM2 13l10 5 10-5",
  fullscreen: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  next: "M5 5l10 7-10 7zM19 5v14",
  star: "M12 3.5l2.6 5.4 5.9.8-4.3 4.1 1 5.8L12 16.8l-5.2 2.8 1-5.8-4.3-4.1 5.9-.8z",
  sort: "M7 4v16M3 16l4 4 4-4M17 20V4M13 8l4-4 4 4",
  grid: "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon(props: { name: IconName; class?: string; size?: number | string; filled?: boolean; title?: string }): JSX.Element {
  const filled = () => props.filled ?? (props.name === "play" || props.name === "pause" || props.name === "next");
  return (
    <svg
      class={"icon" + (props.class ? " " + props.class : "")}
      viewBox="0 0 24 24"
      width={props.size ?? "1em"}
      height={props.size ?? "1em"}
      fill={filled() ? "currentColor" : "none"}
      stroke="currentColor"
      stroke-width={filled() ? 0 : 2}
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden={props.title ? undefined : "true"}
      role={props.title ? "img" : undefined}
    >
      {props.title && <title>{props.title}</title>}
      <path d={PATHS[props.name]} />
    </svg>
  );
}
