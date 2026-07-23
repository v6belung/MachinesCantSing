use serde::Deserialize;

use super::client::MusicBrainzClient;
use crate::text::names_match;

/// Corroborating evidence found on MusicBrainz for an artist who'd otherwise be flagged purely
/// for having no release history before 2025/2026. MusicBrainz is community-edited (not
/// airtight -- see `classifier::has_third_party_corroboration` for the same caveat on the
/// iTunes-side signal), but a documented life-span or a page's worth of external links (official
/// homepage, socials, Discogs, Songkick/Bandsintown/setlist.fm -- MusicBrainz aggregates links to
/// exactly the concert-history sites that would otherwise require their own registered API key)
/// is real community curation that's harder to fake cheaply than self-published streaming volume.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MbCorroboration {
    /// The track was found as a MusicBrainz recording credited to this artist -- distinct from
    /// the two fields below, since a match with neither a life-span nor external links is still
    /// evidence of *some* community documentation, useful for the separate "invisible everywhere"
    /// check (`classifier::is_invisible_everywhere`) even when it doesn't count as corroboration.
    pub found: bool,
    /// MusicBrainz records a formation/birth date for this identity.
    pub has_life_span: bool,
    /// MusicBrainz has at least one external URL relationship on file (homepage, social
    /// media, database entries, tour-date sites, etc.).
    pub has_external_links: bool,
}

impl MbCorroboration {
    pub fn any(self) -> bool {
        self.has_life_span || self.has_external_links
    }
}

#[derive(Debug, Deserialize)]
struct RecordingSearchResponse {
    recordings: Vec<RecordingResult>,
}

#[derive(Debug, Deserialize)]
struct RecordingResult {
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCreditEntry>,
}

#[derive(Debug, Deserialize)]
struct ArtistCreditEntry {
    artist: ArtistCreditArtist,
}

#[derive(Debug, Deserialize)]
struct ArtistCreditArtist {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ArtistLookupResponse {
    #[serde(rename = "life-span")]
    life_span: Option<LifeSpan>,
    #[serde(default)]
    relations: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct LifeSpan {
    begin: Option<String>,
}

/// Search MusicBrainz for a *recording* matching both `track_title` and `artist_name` together
/// -- grounding the match in the actual track being played, the same way iTunes' song-grounded
/// search does, rather than a bare name search. A plain artist-name search was tried first and
/// found to be unsafe for common names: searching "Nika" alone returns five unrelated real
/// people all named exactly "Nika", any of which a name-only match could have silently borrowed
/// life-span/link data from. Grounding by track title avoids that collision; if the specific
/// recording isn't on MusicBrainz at all (common for very new/obscure tracks), this correctly
/// returns no match rather than guessing.
pub async fn lookup_corroboration(
    client: &MusicBrainzClient,
    artist_name: &str,
    track_title: &str,
) -> anyhow::Result<MbCorroboration> {
    let query = format!(
        "recording:\"{}\" AND artist:\"{}\"",
        track_title.replace('"', ""),
        artist_name.replace('"', "")
    );
    let url = format!(
        "https://musicbrainz.org/ws/2/recording/?query={}&fmt=json&limit=10",
        urlencoding::encode(&query)
    );
    let resp: RecordingSearchResponse = client.get_json(&url).await?;
    let Some(matched_id) = resp
        .recordings
        .iter()
        .flat_map(|r| r.artist_credit.iter())
        .find(|ac| names_match(&ac.artist.name, artist_name))
        .map(|ac| ac.artist.id.clone())
    else {
        return Ok(MbCorroboration::default());
    };

    let lookup_url =
        format!("https://musicbrainz.org/ws/2/artist/{matched_id}?inc=url-rels&fmt=json");
    let lookup: ArtistLookupResponse = client.get_json(&lookup_url).await?;

    Ok(MbCorroboration {
        found: true,
        has_life_span: lookup.life_span.as_ref().is_some_and(|l| l.begin.is_some()),
        has_external_links: !lookup.relations.is_empty(),
    })
}
