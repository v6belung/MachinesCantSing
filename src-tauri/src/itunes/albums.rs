use chrono::NaiveDate;
use serde::Deserialize;

use super::client::ItunesClient;

#[derive(Debug, Deserialize)]
struct LookupResponse {
    results: Vec<LookupEntry>,
}

#[derive(Debug, Deserialize)]
struct LookupEntry {
    #[serde(rename = "wrapperType")]
    wrapper_type: Option<String>,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
}

/// One collection (album/single) credited to the looked-up artist. `artist_name` is that
/// collection's own top-level credit, which iTunes sets to whoever the *primary* artist is --
/// for a single where the looked-up artist only appears as a `(feat. ...)` credit, this is a
/// different name (see `classifier::has_third_party_corroboration`).
#[derive(Debug, Clone)]
pub struct AlbumEntry {
    pub release_date: NaiveDate,
    pub artist_name: Option<String>,
}

/// Fetch every album/single for an iTunes artist id (docs/phase0-plan.md §3.1 step 3). The
/// lookup's first result is the artist wrapper itself, not an album — filtered out via
/// wrapperType == "collection". No pagination: iTunes caps this endpoint at `limit`, and an
/// artist who hits that cap has a large back-catalog, which itself means "not flagged".
pub async fn fetch_albums(
    client: &ItunesClient,
    itunes_artist_id: i64,
) -> anyhow::Result<Vec<AlbumEntry>> {
    let url =
        format!("https://itunes.apple.com/lookup?id={itunes_artist_id}&entity=album&limit=200");
    let resp: LookupResponse = client.get_json(&url).await?;
    let entries = resp
        .results
        .into_iter()
        .filter(|e| e.wrapper_type.as_deref() == Some("collection"))
        .filter_map(|e| {
            let release_date = parse_release_date(e.release_date.as_deref()?)?;
            Some(AlbumEntry {
                release_date,
                artist_name: e.artist_name,
            })
        })
        .collect();
    Ok(entries)
}

/// ISO-8601 timestamp with day precision, e.g. "2025-03-14T08:00:00Z" -> the date portion.
fn parse_release_date(s: &str) -> Option<NaiveDate> {
    let date_part = s.get(0..10)?;
    NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()
}
