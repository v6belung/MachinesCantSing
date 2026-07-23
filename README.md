# Now Playing — AI-Artist Flagger

A small Windows tray app that watches your local Spotify session and flags
artists whose earliest release date falls in 2025/2026 — a rough proxy for
"probably an AI-generated act." No login, no API keys, no Spotify account
access of any kind.

Full Phase 1 spec: [`docs/phase0-plan.md`](docs/phase0-plan.md). This README
covers how it works day-to-day and how to build/run it.

## How it works

1. **Now playing, for free.** The app reads whatever the *native Spotify
   desktop client* is already publishing to Windows' System Media Transport
   Controls (SMTC) — the same data that powers the volume flyout and your
   keyboard's media keys. No network call, no Spotify account, no scopes.
   Track changes are pushed instantly via SMTC events.

2. **Classify each new artist once.** The first time an artist is seen, its
   name is looked up on the public, keyless **iTunes Search API**:
   - Search grounded in the actual song playing (`artist + track title`) to
     avoid name collisions, falling back to a plain artist-name search.
   - Fetch that artist's full album list and take the earliest release date.
   - `is_flagged = earliest_release_date.year in {2025, 2026}`.

   The verdict is written **once, permanently**, to a local SQLite database
   (`artist_classification` + `classification_evidence` tables). The same
   artist is never re-classified — a repeat play costs zero network calls.

3. **Tray icon is the only UI surface.** The icon swaps between a neutral and
   a flagged state; the tooltip and right-click menu show the current track,
   artist(s), and flagged status. An optional status window (opened from the
   tray) shows a small history of recent classifications.

iTunes allows ~20 requests/min per IP, so lookups are serialized through a
global rate limiter (~4s spacing) with backoff on 403/5xx — a fresh playlist
full of unseen artists drains gradually rather than bursting.

**Current scope:** Windows only (SMTC). The media backend is behind a
`MediaBackend` trait so a Linux MPRIS backend can be added later without
touching anything above it (see `src-tauri/src/media/mpris.rs`).

## Requirements

- **Windows 11** (SMTC-based; this build does not run on Linux/macOS yet)
- The **native Spotify desktop app**, installed and running (not the web
  player — that doesn't publish to SMTC)
- [Rust](https://rustup.rs/) (stable toolchain, MSVC)
- [Node.js](https://nodejs.org/) 20+ (LTS recommended) and npm
- **Visual Studio Build Tools** with the "Desktop development with C++"
  workload — required for the MSVC linker Rust uses on Windows. Easiest way:
  ```powershell
  winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  ```

No API keys, no `.env` file, no accounts to configure — that's the point.

## Setup

```powershell
git clone https://github.com/v6belung/MachinesCantSing.git
cd MachinesCantSing
npm install
```

### Run in development

```powershell
npm run tauri dev
```

This starts the Vite dev server for the (hidden-by-default) status window
and launches the Tauri app. Open Spotify and play something — after a few
seconds the tray icon should update. Right-click it for the menu.

### Build a release binary

```powershell
npm run tauri build
```

Produces an installer/bundle under `src-tauri/target/release/bundle/`.

### Run the Rust test/lint suite

```powershell
cd src-tauri
cargo test
cargo clippy
```

## Data storage

A local SQLite database is created on first run at:

```
%APPDATA%\dev.v6belung.now-playing-flagger\now-playing-flagger.sqlite3
```

It contains only: normalized artist names, computed artist IDs, flagged
verdicts, and the iTunes evidence used to reach them. Nothing is sent
anywhere except the iTunes Search API lookups described above.

## Known limitations (by design, Phase 1)

- **Placeholder heuristic.** "Earliest release in 2025/2026" is a rough
  signal, not a determination that an artist is AI-generated. It is not
  meant to be improved in this phase.
- **Permanent classification.** Once an artist has a verdict, it's never
  re-checked, even if new evidence would change the answer.
- **Name-only identity.** Two different artists sharing a normalized name
  collapse to one verdict (rare, and the real iTunes artist ID is kept in
  the evidence JSON for a future disambiguation pass).
- **Tray-only UI.** No OS notifications/toasts by design — if Spotify isn't
  running, the tray just shows idle.
