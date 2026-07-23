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

2. **Classify each new artist once.** For a track's credited artist(s), two
   kinds of names are checked — the raw, unsplit credit line (e.g. "Earth,
   Wind & Fire") *and* each individually split name (e.g. "Earth", "Wind",
   "Fire") — since splitting on Spotify's own "A, B & C" convention is a
   heuristic that's wrong for any real artist whose name contains a comma or
   an ampersand. Whichever of those turn out to be genuinely new gets looked
   up on the public, keyless **iTunes Search API**:
   - Search grounded in the actual song playing (`artist + track title`) to
     avoid name collisions, falling back to a plain artist-name search.
   - Fetch that artist's full album list and take the earliest release date.
   - `is_flagged = earliest_release_date.year in {2025, 2026}`.

   Each verdict is written **once, permanently**, to a local SQLite database
   (`artist_classification` + `classification_evidence` tables). The same
   name is never re-classified — a repeat play costs zero network calls. A
   track is flagged overall if *any* of its candidate names comes back
   flagged.

3. **Tray icon is the only UI surface**, with four states: gray (idle,
   nothing playing), hourglass (a candidate is still being classified),
   green (resolved, nothing flagged), red (resolved, flagged). The tooltip
   and right-click menu show the current track, artist(s), and flagged
   status. An optional status window (opened from the tray) shows a small
   history of recent classifications.

iTunes allows ~20 requests/min per IP, so lookups are serialized through a
global rate limiter (~4s spacing) with backoff on 403/5xx — a fresh playlist
full of unseen artists drains gradually rather than bursting.

**Current scope:** Windows only (SMTC). The media backend is behind a
`MediaBackend` trait so a Linux MPRIS backend can be added later without
touching anything above it (see `src-tauri/src/media/mpris.rs`).

## Install (Windows)

No Rust, no Node, no build tools required — just download and run:

1. Go to [Releases](https://github.com/v6belung/MachinesCantSing/releases/latest)
   and download the `.exe` installer for the latest version.
2. Run it. It installs the app and starts it — look for the icon in your
   system tray (it may be hidden in the overflow arrow the first time; drag
   it onto the visible taskbar to keep it there).
3. Open Spotify (the native desktop app, not the web player) and play
   something. The icon updates once the currently playing artist has been
   classified.

You'll need the **native Spotify desktop app** running — the icon just stays
idle if Spotify isn't open, and this only works on **Windows 11** (SMTC-based)
for now. No API keys, no `.env` file, no accounts to configure — that's the
point.

The installer is unsigned (no code-signing certificate yet), so Windows
SmartScreen may warn on first run — "More info" → "Run anyway".

## Versioning & releases

Tags of the form `vX.Y.Z` on this repo trigger
[`.github/workflows/release.yml`](.github/workflows/release.yml), which
builds the Windows installer and publishes it as a GitHub Release. The
version lives in three places that must stay in sync: `package.json`,
`src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

To cut a release:

```powershell
npm run release -- 0.2.0   # bumps all three files, commits, tags
git push && git push origin v0.2.0
```

## Development

Building from source requires:

- [Rust](https://rustup.rs/) (stable toolchain, MSVC)
- [Node.js](https://nodejs.org/) 20+ (LTS recommended) and npm
- **Visual Studio Build Tools** with the "Desktop development with C++"
  workload — required for the MSVC linker Rust uses on Windows. Easiest way:
  ```powershell
  winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  ```

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

### Build a release binary locally

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

### Dependency cooldown policy

Newly published npm package versions haven't had time for the community to
catch a compromised release, so this repo enforces a **7-day cooldown**:
`npm run check:deps` (run in CI on every `package.json`/lockfile change)
fails if any installed npm package version was published less than 7 days
ago. If you need to bump a dependency, prefer the newest version that's
already cleared the cooldown rather than the latest tag.

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
