# Now Playing — AI-Artist Flagger

A small tray app that watches what you're playing in Spotify and flags artists that look
AI-generated. No login, no API keys, no Spotify account access.

## Install

Download from [Releases](https://github.com/v6belung/MachinesCantSing/releases/latest):

- **Windows 11:** run the `.exe` installer. It's unsigned, so SmartScreen may warn: click
  "More info" → "Run anyway".
- **Debian, Ubuntu, Mint:** `sudo apt install ./now-playing-flagger_*.deb`, then launch
  "Now Playing Flagger" from your app menu.
- **Other Linux:** make the `.AppImage` executable (`chmod +x`) and run it.

Then play something in the Spotify desktop app (not the web player). The tray icon updates
once the artist has been checked.

## Reading the icon

| Icon | Meaning |
| --- | --- |
| Gray | Nothing playing |
| Hourglass | Checking the artist |
| Green | Not flagged |
| Amber | Couldn't be checked confidently |
| Red | Flagged as a possible AI artist |

Hover the icon for the current track. Right-click it to see each artist's verdict; open an
artist's submenu to see what led to it (✗ points toward AI, ✓ away from it). The menu also has
a start-on-login option.

## How it works

The current track comes from your OS's media controls, the same source your media keys use.
Each new artist is checked once against the public iTunes and MusicBrainz catalogs, and the
verdict is kept. An artist is flagged when:

- their first release came after March 2024 (when AI music tools got convincing) and nothing
  independent vouches for them, such as another established artist featuring them or a
  documented MusicBrainz entry;
- many releases appeared all at once; or
- neither catalog has heard of them.

On Linux, the app also reads the track's public Spotify page to get the full list of artists,
since Spotify only reports the main artist to the OS.

Treat a flag as "worth a second look", not as proof.

## Privacy

The only network requests are the iTunes, MusicBrainz and (on Linux) Spotify page lookups
above. Nothing about you is sent anywhere. Past verdicts are stored locally:

- Windows: `%APPDATA%\dev.v6belung.now-playing-flagger\`
- Linux: `~/.local/share/dev.v6belung.now-playing-flagger/`

Verdicts are permanent, so improvements in later versions don't re-check artists you've
already heard. To start fresh, quit the app and delete that folder.

## Building from source

Needs [Rust](https://rustup.rs) and Node 22+. On Linux, also install the Tauri system
libraries (Debian/Ubuntu/Mint):

```sh
sudo apt install build-essential libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
```

Then:

```sh
npm ci
npm run tauri dev      # run it
npm run tauri build    # build an installer
```

On GNOME, tray icons need the AppIndicator extension. Cinnamon, KDE and XFCE show them out of
the box.
