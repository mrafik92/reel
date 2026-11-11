# AGENTS.md — working on Reel

Reel is a self-hosted media server: a Rust server in `server/` and a SolidJS web app in `clients/`.
It runs live on the owner's homelab (Proxmox **CT 113**, https://reel.rafikii.co). Read **`docs/`** first:
[`FEATURES`](docs/FEATURES.md), [`ARCHITECTURE`](docs/ARCHITECTURE.md) (includes the design decisions),
[`STATUS`](docs/STATUS.md) and [`OPERATIONS`](docs/OPERATIONS.md).

## 1. Work comes from tickets

All work is tracked on **GitHub Project #1** (https://github.com/users/mrafik92/projects/1). Each card is
an issue in `mrafik92/reel`. When asked to "work on #N":

1. **Read it:** `gh issue view N -R mrafik92/reel --comments`. Don't create a new issue for existing work.
   File new issues only for new problems you discover, using the templates in `.github/ISSUE_TEMPLATE/`.
2. **Move it to Implementing** (see §2 for the command), then create a branch `N-short-slug` from `main`.
3. **Implement it** with tests. Keep commits focused, and mention the ticket in every commit message:
   `Refs #N`.
4. **Validate locally** (see §3) and **update the docs** (see §6) before pushing. Docs changes go in the
   same PR as the code.
5. **Open a PR**: `gh pr create -R mrafik92/reel --base main --head <branch>`, filling in the PR template.
   Write **`Refs #N`, never `Closes #N` / `Fixes #N`**. Then link the PR to the issue in its *Development*
   section, or with `gh pr edit` (the body reference is enough for the board). The project workflow moves
   the card to **Reviewing**.
6. **After merge** (by the owner, or by you when explicitly asked), CI builds the image. To deploy, trigger
   **Actions → deploy → Run workflow** (`gh workflow run deploy -R mrafik92/reel`, only when asked), or run
   `deploy/ct113/deploy.py` from the Mac as a fallback. It moves every ticket referenced in the shipped commits to **Deployed** and
   comments the build SHA. Then run `scripts/sync_obsidian.py` so the owner's Obsidian notes match `docs/`.
7. **Verified** and **Done** belong to the owner. They check the change on the real setup, fill in
   *Verified on*, and close the issue, which moves the card to Done. Don't close tickets yourself
   unless told to.

Why `Refs` and not `Closes`: closing on merge would jump the card straight to Done and skip Deployed and
Verified. Most Reel changes need a check on real media, TVs or the IPTV provider.

## 2. Moving a ticket's status

```sh
P=PVT_kwHOAEJSo84BqIrh                   # project id
F=PVTSSF_lAHOAEJSo84BqIrhzhohHPk         # Status field id
# option ids: Backlog ac7383d3 · To do f80e7288 · Implementing 45fd31a9 · Reviewing 01a8ebed
#             Deployed 0af4576f · Verified 12d1a64c · Done c65bb556
ITEM=$(gh project item-list 1 --owner mrafik92 --format json --limit 500 \
        --jq ".items[] | select(.content.number==N) | .id")
gh project item-edit --id "$ITEM" --project-id $P --field-id $F --single-select-option-id 45fd31a9   # → Implementing
```

Other fields on the board: Area, Priority (P0–Idea), Size (S/M/L), Milestone, and *Verified on* (text).

## 3. Build, test, validate

```sh
# server (rustc 1.86 is pinned in CI; don't use crates that need a newer compiler)
cd server && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings \
          && cargo test --workspace && cargo build --locked -p reel-api
cargo run -q -p reel-api --bin export-openapi > openapi.json   # after any API change; commit it

# clients (bun workspaces; pnpm is not installed)
cd clients && bun install && bun run typecheck && bun run build
# regenerate API types after server API changes:
#   apps/web: src/api/openapi.d.ts from server/openapi.json (see the package scripts)
```

- **Security audit:** `cd server && cargo audit`. Exceptions go in `server/.cargo/audit.toml`, each with a reason; never ignore an advisory without one. Keep sqlx on the single workspace entry (SQLite only); CI fails if a MySQL/Postgres driver or `rsa` gets built.
- **Always commit `server/Cargo.lock`** when dependencies change. The image build uses `--locked` and fails otherwise.
- Tests that generate media need `ffmpeg` (installed locally and in CI).
- `server/target` can grow past 25 GB. Run `cargo clean` if disk space gets low.
- Web UI work: verify in a browser with Playwright, in mock mode (`?mock=1`) and against the live server.
  Check both desktop and **1920×1080 TV mode with arrow-key navigation**.

## 4. Deploying

- Images: GitHub Actions `image.yml` → `ghcr.io/mrafik92/reel:latest` (~10 min for server changes,
  ~1 min for web-only).
- Deploy: **Actions → deploy → Run workflow** (self-hosted runner CT 116, forced-command SSH key). Fallback:
  `deploy/ct113/deploy.py` from the Mac (`--dry-run` first). Both pull, restart, health-check and update tickets.
- Never make the repo public while the self-hosted runner is attached.
- Live access for checks: `ssh minix-local` → `pct exec 113 -- …` (Reel runs in Docker there). Admin
  credentials for API smoke tests are in `/root/.reel-admin` on minix. Never print them.

## 5. Guardrails

- **No secrets** in commits, issues, PRs, logs or chat: provider logins, API keys, tokens or passwords.
  Use hidden prompts (`read -rs`) and root-only files (see `docs/OPERATIONS.md#credentials-and-where-they-live`).
- **Ask before changing infrastructure:** OPNsense, Proxmox host, CT firewall rules, Caddy (CT 107), DNS
  (Technitium) and the Arr stack (CT 105). Read-only inspection is fine.
- Reel stays **private** (LAN + Tailscale) until the security tickets in milestone *M2 – Security gate*
  are done and re-reviewed. Don't add public exposure.
- The IPTV provider rate-limits (HTTP 46x) and allows **one connection**. Don't hammer its API, and don't
  tune channels while someone is watching (`GET /api/v1/admin/iptv/hub`).
- Stage only the files you changed (`git add <paths>`, never `git add -A`). Don't commit `clients/data/`
  or other local artifacts.
- Keep `docs/STATUS.md` honest (see §6).

## 6. Docs are part of every PR

**A PR isn't done until `docs/` describes the code as it is after the merge.** Before opening the PR, go
through this table and update every doc your change affects. If nothing applies, say so in the PR
("Docs: no change, because …").

| If the PR… | Update |
|---|---|
| adds, changes or removes a feature, or changes its status (✅ live / 🧪 built / ⏳ partial) | `docs/FEATURES.md` |
| changes components, crates, data flow, the data model (migrations), the security model, or makes a design choice | `docs/ARCHITECTURE.md` (§5: add or adjust a decision row with *why* and *trade-off*) |
| fixes or opens a gap, changes what's verified, closes or adds security items, or changes the backlog | `docs/STATUS.md` (bump the date and the live-build line when deploying) |
| changes deploy, config/env vars, credential locations, CT 113 compose, firewall needs or runbooks | `docs/OPERATIONS.md` and, if generic, `deploy/README.md` |
| changes the API | `server/openapi.json` (regenerated), plus the client types |
| changes how agents should work | this `AGENTS.md` |

Rules:
- Write what is **true now**, not plans. Plans and ideas belong in tickets.
- Status words must match reality: only mark something ✅ **live** once it's deployed and checked on the real
  setup. Built and tested in CI is 🧪.
- Keep secrets out: name *where* a credential lives, never its value.
- After the merge and deploy, run `scripts/sync_obsidian.py`. It mirrors `docs/` into the owner's Obsidian
  vault (`~/Documents/obsidian/notes-/Reel`). `scripts/sync_obsidian.py --check` reports drift without writing.
- When the ticket moves to Verified, the owner may ask you to flip 🧪 → ✅ in `FEATURES.md`/`STATUS.md`. Do
  that in a small follow-up commit with `Refs #N`.
