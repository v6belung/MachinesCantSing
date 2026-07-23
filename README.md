# Now Playing — AI-Artist Flagger

A small Windows tray app that watches your local Spotify session and flags
artists that look like AI-generated acts — new to streaming with no
independent trace of being real. No login, no API keys, no Spotify account
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
  iTunes catalog and MusicBrainz. Every artist is checked only once, ever —
  the verdict is permanent and reused on every later play. A few signals feed
  into the verdict:
  - An artist credited literally as "Suno" or "Udio" (some AI uploads never
    get renamed) is an instant flag.
  - An artist whose earliest release is from 2025/2026 is flagged *unless*
    something corroborates them as real: a different, established artist
    crediting them as a featured collaborator, or a MusicBrainz entry with a
    documented life-span or external links (official site, socials,
    Discogs, Songkick/Bandsintown/setlist.fm).
  - A sudden burst of many releases in a short window is flagged regardless
    of age — a real artist's catalog rarely appears all at once.
  - An artist found on neither iTunes nor MusicBrainz at all is flagged —
    real artists, however obscure, are rarely invisible to both at once.
- The tray icon has five states: gray (idle, nothing playing), an hourglass
  (checking), green (not flagged), amber (couldn't be confidently checked),
  red (flagged). Hover it for the current track and status, or right-click
  for more detail and an optional history window.
- Right-click the tray icon and check "Start with Windows" if you want the
  app running automatically every time you log in — it's off by default.

Nothing is sent anywhere except those iTunes/MusicBrainz lookups. A local,
private database of past verdicts lives on your machine at
`%APPDATA%\dev.v6belung.now-playing-flagger\`. Verdicts are permanent by
design (an artist is only ever checked once), so if the app's detection
logic improves in a later update, previously-seen artists keep their old
verdict rather than getting re-checked automatically. To force everyone to
be re-evaluated from scratch, close the app and delete that folder (or just
the `.sqlite3` file inside it) before relaunching.

This is a deliberately rough heuristic stack, not a verdict on whether an
artist is really AI-generated — treat a flag as "worth a second look," not
as fact.
