use serde::Deserialize;

use super::client::ItunesClient;
use crate::text::names_match;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMethod {
    SongGrounded,
    ArtistName,
    Unresolved,
}

impl MatchMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchMethod::SongGrounded => "song_grounded",
            MatchMethod::ArtistName => "artist_name",
            MatchMethod::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArtistResolution {
    pub itunes_artist_id: i64,
    pub matched_artist_name: String,
    pub method: MatchMethod,
}

#[derive(Debug, Deserialize)]
struct SearchResponse<T> {
    results: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct SongResult {
    #[serde(rename = "artistId")]
    artist_id: Option<i64>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArtistResult {
    #[serde(rename = "artistId")]
    artist_id: Option<i64>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
}

/// Resolve an artist on iTunes, grounded in the track that's actually playing
/// to cut name-collision risk (docs/phase0-plan.md §3.1 step 2). Falls back to
/// a name-only artist search if no song match is found. Never errors on "not
/// found" — that's a legitimate Unresolved outcome, not a failure.
pub async fn resolve_artist(
    client: &ItunesClient,
    artist_name: &str,
    track_title: &str,
) -> anyhow::Result<Option<ArtistResolution>> {
    if let Some(resolution) = search_song_grounded(client, artist_name, track_title).await? {
        return Ok(Some(resolution));
    }
    search_artist_name(client, artist_name).await
}

async fn search_song_grounded(
    client: &ItunesClient,
    artist_name: &str,
    track_title: &str,
) -> anyhow::Result<Option<ArtistResolution>> {
    let term = format!("{artist_name} {track_title}");
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=song&limit=25",
        urlencoding::encode(&term)
    );
    let resp: SearchResponse<SongResult> = client.get_json(&url).await?;
    let best = resp.results.into_iter().find_map(|r| {
        let (id, name) = (r.artist_id?, r.artist_name?);
        names_match(&name, artist_name).then_some((id, name))
    });
    Ok(
        best.map(|(itunes_artist_id, matched_artist_name)| ArtistResolution {
            itunes_artist_id,
            matched_artist_name,
            method: MatchMethod::SongGrounded,
        }),
    )
}

async fn search_artist_name(
    client: &ItunesClient,
    artist_name: &str,
) -> anyhow::Result<Option<ArtistResolution>> {
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=musicArtist&limit=5",
        urlencoding::encode(artist_name)
    );
    let resp: SearchResponse<ArtistResult> = client.get_json(&url).await?;
    let best = resp.results.into_iter().find_map(|r| {
        let (id, name) = (r.artist_id?, r.artist_name?);
        names_match(&name, artist_name).then_some((id, name))
    });
    Ok(
        best.map(|(itunes_artist_id, matched_artist_name)| ArtistResolution {
            itunes_artist_id,
            matched_artist_name,
            method: MatchMethod::ArtistName,
        }),
    )
}
