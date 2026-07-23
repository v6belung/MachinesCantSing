# Phase 1 Plan — "Now Playing" AI-Artist Flagger

Status: ready for implementation handoff
Scope: Phase 1 only. Do not implement anything beyond what's described here
(no manual override UI, no multi-source verification, no re-classification).

**This app requires NO login and NO API keys of any kind.** Now-playing is
read from the OS media session; artist classification uses keyless public
web lookups. There is no Spotify OAuth, no client secret, no token storage.

---

## 0. Non-negotiable constraints (carried over from spec, repeated here so the
   implementer doesn't have to hold the whole brief in their head)

- Heuristic: artist is flagged iff earliest release date across their
  discography falls in 2025 or 2026. This is a deliberate placeholder — do
  not improve it, do not add secondary signals.
- Classification is **permanent**. Once a row exists in
  `artist_classification` for an artist_id, Phase 1 code never touches it
  again — no re-check on subsequent plays, no TTL, no refresh.
- `locked` column and multi-source evidence shape exist for a future phase.
  Phase 1 always writes `locked = 0` and exactly one
  `classification_evidence` row per artist (`source = 'itunes_search_api'`).
- Cross-platform: Windows 11 + Linux Mint. Tray icon behavior differs
  between the two (see §4.3) — plan for it now.
- **No authentication anywhere** (see §1). Now-playing comes from the OS
  media session (§2); classification comes from the keyless iTunes Search
  API (§3).

---

## 1. No authentication — all data sources are keyless

The two prior designs (Spotify OAuth PKCE; then media-session + PKCE-for-
catalog) are both dropped. Login is gone entirely. Two keyless sources:

1. **Now-playing → OS media session** (SMTC on Windows, MPRIS on Linux).
   The running native Spotify client already publishes track/artist to the
   OS; we read it. No Spotify account access, no scopes, no token. §2.
2. **Classification → iTunes Search API** (`https://itunes.apple.com/...`).
   Public, keyless, no OAuth. Used to look up an artist's albums and their
   release dates to compute the earliest-release proxy. §3.

### 1.1 Why iTunes Search API (and not the Spotify Web API or MusicBrainz)

- **Spotify Web API** has no anonymous tier — every call needs a bearer
  token, which for a *distributed* desktop app means either a per-user login
  (rejected: we're dropping login) or an embedded client secret (rejected:
  extractable from the binary). So Spotify is out for classification.
- **iTunes Search API** is fully keyless, returns a day-precision
  `releaseDate` per album, and — decisively — has strong catalog coverage of
  exactly the population this heuristic targets: brand-new artists pushed to
  streaming in 2025/2026. Rate limit ~20 requests/min per IP; returns HTTP
  403 when exceeded (see §3.4 throttling).
- **MusicBrainz** was evaluated and rejected as the Phase 1 source: it's
  keyless with a cleaner `first-release-date` field, but as a
  community-maintained DB it has thin, lagging coverage of recent/obscure
  artists — i.e. it would return "not found" for many of the very artists we
  need to date. It is, however, the natural **second** evidence source for
  the FUTURE multi-source verification phase (which is what the
  `classification_evidence` table's multi-row design is for). Do not
  implement MusicBrainz in Phase 1.

### 1.2 iTunes Search API — required client behavior

- **No auth headers.** Just HTTPS GET requests.
- Set a descriptive `User-Agent` (good web citizenship; some Apple edge
  nodes 403 empty/blank UAs).
- Endpoints used (both documented, keyless):
  - `GET https://itunes.apple.com/search?term=<q>&entity=<...>&limit=<n>`
  - `GET https://itunes.apple.com/lookup?id=<artistId>&entity=album&limit=200`
- Respect the ~20 req/min ceiling with a global throttle (§3.4).

---

## 2. Now-playing detection via OS media session (event-driven, no network)

Now-playing is read from the running native Spotify client's OS media
session — the same data that powers the OS media flyout and hardware media
keys. Event-driven (push on track change), zero network, updates instantly.

### 2.1 Per-OS media backends

- **Windows 11 — System Media Transport Controls (SMTC).** Use
  `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`
  via the `windows` crate. Enumerate sessions, pick the Spotify session,
  read `GetMediaProperties()` → `Title`, `Artist`, `AlbumTitle`. Subscribe
  to `MediaPropertiesChanged` / `CurrentSessionChanged` for push updates.
- **Linux Mint — MPRIS over D-Bus.** Talk to
  `org.mpris.MediaPlayer2.spotify` on the session bus (via `zbus`), object
  `/org/mpris/MediaPlayer2`, interface `org.mpris.MediaPlayer2.Player`.
  Read the `Metadata` property (`a{sv}`): `xesam:artist`, `xesam:title`,
  `xesam:album`. Subscribe to `PropertiesChanged` for push updates.

Both backends sit behind one internal `MediaSession` trait so the rest of
the app consumes one normalized event regardless of platform:

```
NowPlaying {
    track_title: String,
    artist_names: Vec<String>,   // a track can credit multiple artists
    playback_status: Playing | Paused | Stopped,
}
```

Note: neither backend needs Spotify IDs anymore — classification (§3) works
purely from the artist name (grounded by the track title). SMTC never
exposed IDs anyway; this design no longer cares.

### 2.2 Behavior

- **Driven by media-session change events, not a timer.** On an event with
  no active track (stopped/paused, empty), set the tray to "idle."
- **No network polling of any kind here.** If a backend can't deliver
  reliable change events on some system, fall back to a low-frequency
  *local* read of the media session (every 2–3s) — a local WinRT/D-Bus read,
  never a network call. Prefer event subscription; local polling is a
  compatibility fallback only.
- **Spotify client not running / no session:** tray shows idle; no errors.

### 2.3 Dedup — don't re-classify artists we've already seen

Identity: since we no longer have any Spotify/track ID, the only stable
cross-platform identity is the **artist name**. Derive the primary key:

```
artist_id = "name:" + normalize(artist_name)
normalize = lowercase, trim, collapse internal whitespace, strip diacritics
```

This makes the PK computable offline from the media event alone, which
restores the clean property that **the `artist_classification` PK lookup
IS the dedup cache** (no separate "seen" set needed).

On each now-playing event with an active track, for each artist name:

1. Compute `artist_id` as above.
2. PK lookup in `artist_classification`.
   a. **Hit** → skip entirely (no network, no evidence write, no
      re-evaluation). This is what makes classification permanent, and it
      means a repeat play of a known artist costs zero iTunes calls.
   b. **Miss** → enqueue `{artist_id, artist_name, track_title}` for the
      classification pipeline (§3). `track_title` is carried so the pipeline
      can ground the iTunes artist match in the actual song being played.
3. Update the tray from `artist_classification` (including rows just written).

**Accepted limitation (placeholder-phase risk):** two genuinely different
artists sharing a normalized name collapse to one `artist_id` and share one
verdict. This is inherent to name-only identity and acceptable for the
placeholder heuristic; the real iTunes numeric `artistId` is still recorded
in the evidence JSON (§3.3) so a future phase can disambiguate.

**Concurrency guard:** an in-memory `HashSet<artist_id>` of "classification
in flight," checked before step 2b enqueues and cleared when the pipeline
finishes (success or failure). The DB PK remains the source of truth — a
duplicate `INSERT` would fail loudly if the guard ever missed, which is
correct (never silently overwrite).

---

## 3. Artist classification pipeline (iTunes Search API)

Triggered per never-before-seen `artist_id` from §2.3, with the
`artist_name` and the `track_title` that surfaced it.

### 3.1 Steps

1. **Cache check** — already done in §2.3 (a miss is why we're here).
2. **Resolve the artist on iTunes** (grounded in the played track to cut
   name-collision risk):
   - Primary: `GET /search?term=<url(artist_name + " " + track_title)>
     &entity=song&limit=25`. Find the returned song whose `artistName`
     best-matches `artist_name` (case/diacritic-insensitive); take its
     `artistId`. This anchors the match to the actual song playing.
   - Fallback (no song match): `GET /search?term=<url(artist_name)>
     &entity=musicArtist&limit=5`; take the best `artistName` match's
     `artistId`.
   - If neither yields a confident match → treat as **unresolved** (step 5
     writes a low-confidence, not-flagged verdict; do NOT skip writing).
3. **Fetch the artist's albums**:
   `GET /lookup?id=<artistId>&entity=album&limit=200`.
   - The first result is the artist wrapper; the rest are album collections.
   - Collect every collection's `releaseDate`.
   - Note: an artist-id-scoped album lookup returns that artist's *own*
     releases. Various-Artists compilations that merely feature the artist
     are filed under the Various Artists id, so they don't contaminate this
     result — this naturally avoids the "appears_on / compilation" problem
     the earlier Spotify design had to filter out by hand. No `include_groups`
     equivalent needed.
   - iTunes returns up to 200 in one call; artists exceeding that are
     astronomically rare for a "recent debut" signal. If `resultCount`
     equals the limit, that itself indicates a large back-catalog (=> old
     artist => not flagged); no pagination needed for the heuristic.
4. **Earliest-date calculation**:
   - `releaseDate` is an ISO-8601 timestamp with day precision
     (e.g. `"2025-03-14T08:00:00Z"`). Parse the date portion → `YYYY-MM-DD`.
     No year/month precision ambiguity to handle (simpler than Spotify's
     `release_date_precision`).
   - `earliest_release_date = min(all parsed album releaseDates)`.
   - `is_flagged = earliest_release_date.year in {2025, 2026}`.
5. **Zero-data / unresolved edge cases** — still write exactly one permanent
   verdict (the data-model contract: every artist encountered gets one row):
   - Unresolved artist (step 2 found nothing) OR resolved but zero albums
     returned → `earliest_release_date = NULL`, `is_flagged = 0`,
     `confidence` = low (see §3.3). Never crash, never skip the write.
6. **DB writes** (single `BEGIN`/`COMMIT` over both tables so a crash never
   leaves one written without the other):
   - `INSERT INTO artist_classification (artist_id, artist_name, is_flagged,
     classified_at, method, confidence, earliest_release_date, locked)`
     with `method = 'itunes_search_api'`, `locked = 0`.
   - `INSERT INTO classification_evidence (artist_id, source, result,
     supports_ai, recorded_at)` with `source = 'itunes_search_api'`,
     `supports_ai = is_flagged`, and `result` = the evidence JSON (§3.3).
   - On any failure, roll back; the in-flight guard (§2.3) clears regardless
     so a later event can retry.

### 3.2 Confidence tiers (stored in `artist_classification.confidence`)

Exact numeric scale is the implementer's call; the ordering is what matters:
- **High** — artist matched via the track-grounded song search AND ≥1 album
  dated.
- **Medium** — artist matched only via the name-only artist search fallback.
- **Low** — unresolved on iTunes, or resolved with zero albums (verdict is a
  not-flagged default, flagged as weak evidence for the future phase).

### 3.3 Evidence JSON (`classification_evidence.result`)

Free-form JSON summarizing what the check found — recorded for the future
multi-source verification phase. Suggested shape:

```json
{
  "itunes_artist_id": 1234567890,
  "matched_artist_name": "Actual Matched Name",
  "match_method": "song_grounded | artist_name | unresolved",
  "album_count": 3,
  "earliest_release_date": "2025-03-14",
  "queried_track_title": "Song That Was Playing"
}
```

### 3.4 Throttling & error handling (iTunes)

- iTunes allows **~20 requests/min per IP**; over that it returns **HTTP
  403**. Each classification uses 1–2 requests.
- Wrap the iTunes HTTP client in a **global rate limiter**: a single-worker
  serial queue with a minimum spacing of ~4s between HTTP calls (≈15/min,
  safely under the ceiling). Classification requests queue behind it — a
  fresh playlist full of unseen artists drains gradually rather than
  bursting.
- On **403** (rate limited): back off ~60s, then resume the queue. Do not
  drop the queued item.
- On transient network/5xx: exponential backoff (start 5s, cap 60s), retry
  the same item. On persistent failure, write the low-confidence unresolved
  verdict (§3.1 step 5) rather than looping forever.

### 3.5 Where this lives

All Rust. The frontend only receives the resulting row via a Tauri event so
the tray can update.

---

## 4. Tauri project skeleton

### 4.1 Command / event structure (Rust ⇄ frontend boundary)

**Rust owns:** the OS media-session monitor, iTunes lookups + throttling,
the classification pipeline, all SQLite access, the tray. There is no auth
to own anymore. Media-session access (WinRT SMTC / D-Bus MPRIS) is inherently
native and can only live in Rust.

**Frontend owns:** a small optional status window (opened from the tray) —
current track + its artists' flagged state + a short recent-classifications
list. No login UI (there's no login). The window is not shown on startup.

Tauri commands (frontend → Rust):
- `get_current_state()` — last-known now-playing snapshot + flagged status of
  its artists (so the window isn't blank between events).
- `get_recent_classifications(limit)` — last N rows from
  `artist_classification` for the status window.

Tauri events (Rust → frontend):
- `now-playing-changed` — payload: `track_title` + artist list, each
  `{ artist_id, name, is_flagged }`. `is_flagged` is `null`/"pending" while
  a brand-new artist is still being classified; a follow-up event resolves it.
- `classification-error` (optional) — for a debug log panel; not required.

(No `start_login` / `get_auth_status` / `logout` / `auth-status-changed` —
all removed with the auth system.)

### 4.2 Rust module layout (backend, `src-tauri/src/`)

- `media` — the OS media-session monitor: a `MediaSession` trait with two
  backends (`smtc.rs` Windows via `windows` crate, `mpris.rs` Linux via
  `zbus`), emitting the normalized `NowPlaying` event. Owns the
  name-normalization → `artist_id`, the dedup PK check, and the in-flight
  guard. Emits `now-playing-changed`.
- `itunes` — keyless iTunes Search API client: `client.rs` (HTTP +
  User-Agent + the global rate limiter/queue + 403 backoff), `search.rs`
  (song-grounded + artist-name resolution → `artistId`), `albums.rs`
  (`/lookup` album fetch → release dates).
- `classifier` — the §3 pipeline: resolve → fetch albums → earliest-date →
  confidence → dual-table write. Callable with `{artist_id, artist_name,
  track_title}`.
- `db` — SQLite connection + migration runner + typed queries for both
  tables. Direct `rusqlite` (or `sqlx` sqlite feature) from Rust, **not**
  `tauri-plugin-sql`: the frontend never touches SQL, all access is inside
  Rust media/classifier/command code, so a direct crate avoids a pointless
  IPC hop per query.
- `tray` — tray icon + menu + tooltip setup and update (§4.3).
- `commands` — the `#[tauri::command]` fns from §4.1, thin glue.

### 4.3 Tray icon UI approach, Windows vs Linux

**Surfacing decision (locked):** the tray icon is the *only* surface in
Phase 1. No OS notifications/toasts, no `tauri-plugin-notification`, no
always-on-top overlay. Do not add any of these on your own initiative. The
Windows 11 behavior where new tray icons hide in the overflow flyout by
default (user drags the icon onto the taskbar once) is a **known and
accepted** limitation — do not work around it with a fallback notification.

Both platforms use Tauri v2's built-in `tray-icon` API (`TrayIconBuilder`) —
one code path. Details to design around:

- **Icon swap = the flagged signal**: swap the tray icon image between a
  "neutral" and a "flagged" variant (two small PNG/ICO assets bundled in the
  binary) when the currently-playing artist's flagged state changes.
  Identical on both OSes; sidesteps the non-unified per-OS badge/overlay APIs.
- **Tooltip**: `TrayIcon::set_tooltip` → current track + artist + flagged
  status. Supported on both.
- **Menu**: left-click-to-open-menu is unsupported on Linux per Tauri docs —
  so always expose info via the standard (right-click) tray menu, not a
  custom left-click popup. Items: current track (label-only), flagged status
  (label-only), separator, "Open status window", separator, "Quit".
  (No "Log out" item — there's no login.)

### 4.4 Frontend

Minimal: one optional status window (opened from the tray, hidden on
startup) showing the current track, its artists' flagged state, and a short
recent-classifications list. Keep the web frontend deliberately small.

---

## 5. File / module layout

```
now-playing-flagger/
├── docs/
│   └── phase0-plan.md              (this file)
├── src-tauri/
│   ├── Cargo.toml                  (deps: tauri, rusqlite/sqlx, reqwest,
│   │                                 serde, windows (win), zbus (linux);
│   │                                 NO oauth/keyring deps)
│   ├── tauri.conf.json             (tray config, identifier, status window)
│   ├── icons/
│   │   ├── tray-neutral.png
│   │   └── tray-flagged.png
│   ├── migrations/
│   │   └── 0001_init.sql           (both tables — DDL in §5.1)
│   └── src/
│       ├── main.rs                 (Tauri builder, plugin registration,
│       │                             wires up tray + media monitor startup)
│       ├── media/
│       │   ├── mod.rs              (MediaSession trait, normalized event,
│       │   │                         name→artist_id, dedup, in-flight guard)
│       │   ├── smtc.rs             (Windows SMTC backend, `windows` crate)
│       │   └── mpris.rs            (Linux MPRIS/D-Bus backend, `zbus`)
│       ├── itunes/
│       │   ├── mod.rs
│       │   ├── client.rs           (HTTP, User-Agent, rate limiter/queue,
│       │   │                         403 backoff)
│       │   ├── search.rs           (song-grounded + name artist resolution)
│       │   └── albums.rs           (/lookup album fetch → release dates)
│       ├── classifier/
│       │   └── mod.rs              (§3 pipeline)
│       ├── db/
│       │   ├── mod.rs              (connection setup, migration runner)
│       │   ├── artist_classification.rs
│       │   └── classification_evidence.rs
│       ├── tray/
│       │   └── mod.rs              (icon/menu/tooltip setup + update fns)
│       └── commands.rs             (#[tauri::command] fns from §4.1)
└── src/                             (frontend — status window only)
    ├── main.ts
    ├── index.html
    └── now-playing.ts               (listens for now-playing-changed,
                                       calls get_current_state /
                                       get_recent_classifications)
```

### 5.1 `0001_init.sql` (schema exactly as specified — do not redesign)

```sql
CREATE TABLE artist_classification (
    artist_id             TEXT PRIMARY KEY,        -- "name:" + normalized name
    artist_name           TEXT NOT NULL,           -- original display name
    is_flagged            INTEGER NOT NULL,        -- 0/1
    classified_at         TEXT NOT NULL,           -- ISO8601
    method                TEXT NOT NULL,           -- 'itunes_search_api'
    confidence            REAL,
    earliest_release_date TEXT,                    -- ISO8601 date, nullable
    locked                INTEGER NOT NULL DEFAULT 0  -- unused in Phase 1
);

CREATE TABLE classification_evidence (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    artist_id     TEXT NOT NULL,
    source        TEXT NOT NULL,       -- 'itunes_search_api' in Phase 1
    result        TEXT NOT NULL,       -- evidence JSON (§3.3)
    supports_ai   INTEGER NOT NULL,    -- 0/1, mirrors is_flagged for this check
    recorded_at   TEXT NOT NULL,       -- ISO8601
    FOREIGN KEY (artist_id) REFERENCES artist_classification(artist_id)
);
```

The schema is unchanged from the original spec. Only the *values* written to
`method` / `source` differ (`itunes_search_api`), and `artist_id` is now a
name-derived key instead of a Spotify ID — both are data-level choices the
schema already accommodates (`artist_id`/`method`/`source` are all `TEXT`).

---

## 6. Open judgment calls flagged for confirmation (not blocking, but worth a look)

1. **iTunes as the classification source** (§1.1) — chosen over MusicBrainz
   for coverage of recent artists; MusicBrainz is earmarked as the future
   phase's second evidence source. This changes the data *source* behind the
   heuristic, not the 2025/2026 cutoff itself.
2. **Name-only artist identity** (§2.3) — `artist_id = "name:" + normalized
   name`. Forced by dropping Spotify IDs. Two distinct artists with the same
   name collapse to one verdict; accepted for the placeholder phase, with the
   real iTunes id kept in evidence JSON for later disambiguation.
3. **Windows/Linux match parity** — both platforms now resolve the artist the
   same way (iTunes song-grounded search from name+track), so there's no
   per-OS matching asymmetry anymore. The earlier Windows-search-fuzziness
   caveat is superseded; the only residual fuzziness is name matching, which
   the track-grounded search mitigates.

Everything else implements the spec (heuristic, permanence, data model,
dual-table write, tray-only surfacing) as given.

(The two decided items — tray-only surfacing, and the keyless/no-login
architecture — are settled, not open.)
