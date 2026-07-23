import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface ArtistFlag {
  artist_id: string;
  name: string;
  is_flagged: boolean | null;
}

export interface NowPlayingChanged {
  track_title: string;
  artists: ArtistFlag[];
}

export interface RecentClassification {
  artist_id: string;
  artist_name: string;
  is_flagged: boolean;
  classified_at: string;
  confidence: number | null;
  earliest_release_date: string | null;
}

function badge(isFlagged: boolean | null): string {
  if (isFlagged === null) return `<span class="badge pending">pending</span>`;
  return isFlagged
    ? `<span class="badge flagged">flagged</span>`
    : `<span class="badge clear">clear</span>`;
}

function renderNowPlaying(state: NowPlayingChanged | null): void {
  const titleEl = document.getElementById("track-title")!;
  const listEl = document.getElementById("artist-list")!;

  if (!state) {
    titleEl.textContent = "Nothing playing";
    listEl.innerHTML = "";
    return;
  }

  titleEl.textContent = state.track_title;
  listEl.innerHTML = state.artists
    .map((a) => `<li>${badge(a.is_flagged)} ${escapeHtml(a.name)}</li>`)
    .join("");
}

function renderRecent(rows: RecentClassification[]): void {
  const listEl = document.getElementById("recent-list")!;
  listEl.innerHTML = rows
    .map(
      (r) =>
        `<li>${badge(r.is_flagged)} ${escapeHtml(r.artist_name)}</li>`,
    )
    .join("");
}

function escapeHtml(s: string): string {
  const div = document.createElement("div");
  div.textContent = s;
  return div.innerHTML;
}

async function refreshRecent(): Promise<void> {
  const rows = await invoke<RecentClassification[]>(
    "get_recent_classifications",
    { limit: 20 },
  );
  renderRecent(rows);
}

export async function initNowPlaying(): Promise<void> {
  const initial = await invoke<NowPlayingChanged | null>("get_current_state");
  renderNowPlaying(initial);
  await refreshRecent();

  await listen<NowPlayingChanged>("now-playing-changed", (event) => {
    renderNowPlaying(event.payload);
    void refreshRecent();
  });
}
