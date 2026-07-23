use chrono::{Datelike, NaiveDate, Utc};

use crate::db::Db;
use crate::db::artist_classification::NewClassification;
use crate::db::classification_evidence::NewEvidence;
use crate::itunes::search::MatchMethod;
use crate::itunes::{ItunesClient, albums, search};
use crate::text::normalize_artist_name;

/// Credited-artist names that AI generators use as their own attribution when the uploader
/// doesn't bother renaming it (docs "how to detect AI audio": Udio/Suno tracks are credited
/// to the tool itself unless manually changed). An exact match is an instant, high-confidence
/// flag -- no iTunes lookup needed.
const AI_TOOL_DENYLIST: &[&str] = &["suno", "udio"];

/// A back-catalog that appears all at once rather than accumulating over months/years is
/// itself a signal, independent of *when* it appeared -- AI/farm accounts commonly dump many
/// tracks/albums in one sitting. Thresholds are deliberately conservative (a legitimate
/// prolific artist doing a multi-album release day is rare but possible) since this ORs
/// straight into `is_flagged` alongside the earliest-release-date check.
const BURST_MIN_ALBUMS: usize = 5;
const BURST_MAX_SPAN_DAYS: i64 = 30;

/// Confidence tiers (docs/phase0-plan.md §3.2). Ordering is what matters, not the exact scale.
#[derive(Debug, Clone, Copy)]
enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    fn as_f64(self) -> f64 {
        match self {
            Confidence::High => 1.0,
            Confidence::Medium => 0.6,
            Confidence::Low => 0.2,
        }
    }
}

/// Any stored confidence at or below this is the "Low" tier -- unresolved on iTunes, or
/// resolved with zero albums. Exposed so `media`/`tray` can surface that distinctly from a
/// confidently-checked "not flagged" verdict, without duplicating the tier's numeric value.
pub const LOW_CONFIDENCE_MAX: f64 = 0.25;

pub struct ClassifyRequest {
    pub artist_id: String,
    pub artist_name: String,
    pub track_title: String,
}

fn is_ai_tool_denylisted(artist_name: &str) -> bool {
    let normalized = normalize_artist_name(artist_name);
    AI_TOOL_DENYLIST.contains(&normalized.as_str())
}

fn is_release_burst(dates: &[NaiveDate]) -> bool {
    if dates.len() < BURST_MIN_ALBUMS {
        return false;
    }
    let earliest = *dates.iter().min().expect("checked non-empty above");
    let latest = *dates.iter().max().expect("checked non-empty above");
    (latest - earliest).num_days() <= BURST_MAX_SPAN_DAYS
}

/// True if any of the artist's iTunes collections is credited to a *different* primary artist
/// name -- i.e. this artist shows up as a featured collaborator on someone else's release
/// (e.g. "Fliegen (feat. Tiziano Guerzoni)" credited to "G Kollektiv"). A separate, real
/// catalog choosing to credit them is much harder to fake cheaply than solo-published volume,
/// so it's treated as strong evidence against "this is a synthetic persona."
fn has_third_party_corroboration(artist_name: &str, entries: &[albums::AlbumEntry]) -> bool {
    let normalized = normalize_artist_name(artist_name);
    entries.iter().any(|e| {
        e.artist_name
            .as_deref()
            .is_some_and(|name| normalize_artist_name(name) != normalized)
    })
}

/// The full §3 pipeline: resolve -> fetch albums -> earliest-date -> confidence -> dual-table
/// write. Never skips the write, even when unresolved or zero-album (docs/phase0-plan.md §3.1
/// step 5) -- every artist encountered gets exactly one permanent row.
pub async fn classify(
    db: &Db,
    itunes: &ItunesClient,
    req: ClassifyRequest,
) -> anyhow::Result<bool> {
    if is_ai_tool_denylisted(&req.artist_name) {
        let now = Utc::now().to_rfc3339();
        let evidence = serde_json::json!({
            "denylisted_name": req.artist_name,
            "queried_track_title": req.track_title,
        });
        db.write_classification_with_evidence(
            &NewClassification {
                artist_id: &req.artist_id,
                artist_name: &req.artist_name,
                is_flagged: true,
                classified_at: &now,
                method: "artist_name_denylist",
                confidence: Some(Confidence::High.as_f64()),
                earliest_release_date: None,
            },
            &NewEvidence {
                artist_id: &req.artist_id,
                source: "artist_name_denylist",
                result: &evidence.to_string(),
                supports_ai: true,
                recorded_at: &now,
            },
        )?;
        return Ok(true);
    }

    let resolution = match search::resolve_artist(itunes, &req.artist_name, &req.track_title).await
    {
        Ok(resolution) => resolution,
        Err(err) => {
            log::warn!(
                "iTunes artist resolution failed for '{}': {err:?}",
                req.artist_name
            );
            None
        }
    };

    let method = resolution
        .as_ref()
        .map(|r| r.method)
        .unwrap_or(MatchMethod::Unresolved);

    let (earliest, album_count, burst, corroborated) = match &resolution {
        Some(r) => match albums::fetch_albums(itunes, r.itunes_artist_id).await {
            Ok(entries) => {
                let dates: Vec<NaiveDate> = entries.iter().map(|e| e.release_date).collect();
                (
                    dates.iter().min().copied(),
                    dates.len(),
                    is_release_burst(&dates),
                    has_third_party_corroboration(&req.artist_name, &entries),
                )
            }
            Err(err) => {
                log::warn!(
                    "iTunes album lookup failed for '{}' (itunes_artist_id={}): {err:?}",
                    req.artist_name,
                    r.itunes_artist_id
                );
                (None, 0, false, false)
            }
        },
        None => (None, 0, false, false),
    };

    let confidence = match (method, earliest.is_some()) {
        (MatchMethod::SongGrounded, true) => Confidence::High,
        (MatchMethod::ArtistName, _) => Confidence::Medium,
        _ => Confidence::Low,
    };

    // A brand-new release history is expected for every real emerging artist too, so it's
    // necessary but not sufficient. Corroboration overrides it specifically (rather than
    // overriding `burst`, a stronger and more independent signal): a real, distinct other
    // artist choosing to credit this name as a collaborator is hard to fake cheaply, unlike
    // self-published volume.
    let new_artist = earliest
        .map(|d| matches!(d.year(), 2025 | 2026))
        .unwrap_or(false);
    let is_flagged = (new_artist && !corroborated) || burst;

    let now = Utc::now().to_rfc3339();
    let earliest_str = earliest.map(|d| d.to_string());

    let evidence = serde_json::json!({
        "itunes_artist_id": resolution.as_ref().map(|r| r.itunes_artist_id),
        "matched_artist_name": resolution.as_ref().map(|r| r.matched_artist_name.clone()),
        "match_method": method.as_str(),
        "album_count": album_count,
        "earliest_release_date": earliest_str,
        "release_burst": burst,
        "third_party_corroboration": corroborated,
        "queried_track_title": req.track_title,
    });

    db.write_classification_with_evidence(
        &NewClassification {
            artist_id: &req.artist_id,
            artist_name: &req.artist_name,
            is_flagged,
            classified_at: &now,
            method: "itunes_search_api",
            confidence: Some(confidence.as_f64()),
            earliest_release_date: earliest_str.as_deref(),
        },
        &NewEvidence {
            artist_id: &req.artist_id,
            source: "itunes_search_api",
            result: &evidence.to_string(),
            supports_ai: is_flagged,
            recorded_at: &now,
        },
    )?;

    Ok(is_flagged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn denylist_matches_exact_name_case_and_accent_insensitively() {
        assert!(is_ai_tool_denylisted("Suno"));
        assert!(is_ai_tool_denylisted("udio"));
        assert!(is_ai_tool_denylisted("  UDIO  "));
    }

    #[test]
    fn denylist_does_not_match_unrelated_or_substring_names() {
        assert!(!is_ai_tool_denylisted("Sunology"));
        assert!(!is_ai_tool_denylisted("Studio Killers"));
        assert!(!is_ai_tool_denylisted("Some Artist"));
    }

    #[test]
    fn burst_requires_minimum_album_count() {
        let dates: Vec<NaiveDate> = (1..=4).map(|d| date(&format!("2024-01-{d:02}"))).collect();
        assert!(!is_release_burst(&dates));
    }

    #[test]
    fn burst_detects_many_albums_in_a_short_window() {
        let dates: Vec<NaiveDate> = (1..=6).map(|d| date(&format!("2024-01-{d:02}"))).collect();
        assert!(is_release_burst(&dates));
    }

    #[test]
    fn no_burst_when_albums_span_a_long_time() {
        let dates = vec![
            date("2018-01-01"),
            date("2019-06-01"),
            date("2020-11-01"),
            date("2022-03-01"),
            date("2024-07-01"),
        ];
        assert!(!is_release_burst(&dates));
    }

    fn entry(release_date: &str, artist_name: Option<&str>) -> albums::AlbumEntry {
        albums::AlbumEntry {
            release_date: date(release_date),
            artist_name: artist_name.map(str::to_string),
        }
    }

    #[test]
    fn corroborated_when_credited_under_a_different_primary_artist() {
        let entries = vec![
            entry("2025-01-13", Some("Tiziano Guerzoni")),
            entry("2026-04-05", Some("Dj dodotech")),
        ];
        assert!(has_third_party_corroboration("Tiziano Guerzoni", &entries));
    }

    #[test]
    fn not_corroborated_when_every_collection_is_self_credited() {
        let entries = vec![
            entry("2025-01-13", Some("Tiziano Guerzoni")),
            entry("2025-03-28", Some("tiziano guerzoni")),
        ];
        assert!(!has_third_party_corroboration("Tiziano Guerzoni", &entries));
    }

    #[test]
    fn not_corroborated_when_artist_name_is_missing() {
        let entries = vec![entry("2025-01-13", None)];
        assert!(!has_third_party_corroboration("Tiziano Guerzoni", &entries));
    }
}
