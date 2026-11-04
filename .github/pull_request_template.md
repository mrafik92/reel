## What & why

Refs #

<!-- Use "Refs #N", not "Closes/Fixes #N": the ticket must pass through Deployed → Verified on the real setup. -->

## Changes

-

## Validation

- [ ] `cargo fmt --check` · `clippy -D warnings` · `cargo test --workspace` · `cargo build --locked -p reel-api`
- [ ] `openapi.json` regenerated (if the API changed) and client types updated
- [ ] `bun run typecheck` · `bun run build` (if clients changed)
- [ ] UI checked in a browser: desktop + 1920×1080 TV mode (arrow keys)
- [ ] No secrets, local data or unrelated files in the diff; `Cargo.lock` included if deps changed

## Docs (AGENTS.md §6)

- [ ] `docs/FEATURES.md` / `ARCHITECTURE.md` / `STATUS.md` / `OPERATIONS.md` updated where affected, or: *no doc change because …*
- [ ] Status markers honest (✅ only when verified on the real setup)
- [ ] After merge + deploy: `scripts/sync_obsidian.py` run

## How to verify on the real setup (after deploy)

<!-- What the owner should check on CT 113 / real media / TVs / IPTV to move the ticket to Verified. -->
