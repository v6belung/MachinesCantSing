# Now Playing — AI-Artist Flagger

A small Windows tray app that watches your local Spotify session and flags
artists whose earliest release date falls in 2025/2026 — a rough proxy for
"probably an AI-generated act." No login, no API keys, no Spotify account
access of any kind.

## Install

1. Go to [Releases](https://github.com/v6belung/MachinesCantSing/releases/latest)
   and download the `.exe` installer for the latest version.
2. Run it. It installs the app and starts it — look for the icon in your
   system tray (it may be hidden in the overflow arrow the first time; drag
   it onto the visible taskbar to keep it there).
3. Open Spotify (the native desktop app, not the web player) and play
   something. The icon updates once the currently playing artist has been
   classified.

The installer is unsigned, so Windows SmartScreen may warn on first run —
click "More info" → "Run anyway".

**Requirements:** Windows 11, with the native Spotify desktop app installed
and running. No accounts, API keys, or `.env` files to set up — that's the
point.

## How it works

- Now-playing comes straight from Windows' System Media Transport Controls
  (the same info that powers your media keys) — no Spotify login, no network
  call to Spotify at all.
- The first time an artist is seen, their name is checked against the public
  iTunes catalog for their earliest release date. If that's 2025 or 2026,
  they're flagged. Every artist is checked only once, ever — the verdict is
  permanent and reused on every later play.
- The tray icon has four states: gray (idle, nothing playing), an hourglass
  (checking), green (not flagged), red (flagged). Hover it for the current
  track and status, or right-click for more detail and an optional history
  window.

Nothing is sent anywhere except those iTunes lookups. A local, private
database of past verdicts lives on your machine at
`%APPDATA%\dev.v6belung.now-playing-flagger\`.

This is a deliberately rough, placeholder heuristic, not a verdict on
whether an artist is really AI-generated — treat a flag as "worth a second
look," not as fact.

Full technical design: [`docs/phase0-plan.md`](docs/phase0-plan.md).
