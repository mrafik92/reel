import { init } from "@noriginmedia/norigin-spatial-navigation";

let done = false;
/** Initialise D-pad / remote spatial navigation (idempotent). */
export function initTvNavigation(opts: { debug?: boolean } = {}): void {
  if (done) return;
  done = true;
  init({ debug: opts.debug ?? false, visualDebug: false });
}
