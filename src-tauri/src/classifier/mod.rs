use chrono::{Datelike, Utc};

use crate::db::Db;
use crate::db::artist_classification::NewClassification;
use crate::db::classification_evidence::NewEvidence;
use crate::itunes::search::MatchMethod;
use crate::itunes::{ItunesClient, albums, search};

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

pub struct ClassifyRequest {
    pub artist_id: String,
    pub artist_name: String,
    pub track_title: String,
}

/// The full §3 pipeline: resolve -> fetch albums -> earliest-date -> confidence -> dual-table
/// write. Never skips the write, even when unresolved or zero-album (docs/phase0-plan.md §3.1
/// step 5) -- every artist encountered gets exactly one permanent row.
pub async fn classify(
    db: &Db,
    itunes: &ItunesClient,
    req: ClassifyRequest,
) -> anyhow::Result<bool> {
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

    let (earliest, album_count) = match &resolution {
        Some(r) => match albums::fetch_release_dates(itunes, r.itunes_artist_id).await {
            Ok(dates) => (dates.iter().min().copied(), dates.len()),
            Err(err) => {
                log::warn!(
                    "iTunes album lookup failed for '{}' (itunes_artist_id={}): {err:?}",
                    req.artist_name,
                    r.itunes_artist_id
                );
                (None, 0)
            }
        },
        None => (None, 0),
    };

    let confidence = match (method, earliest.is_some()) {
        (MatchMethod::SongGrounded, true) => Confidence::High,
        (MatchMethod::ArtistName, _) => Confidence::Medium,
        _ => Confidence::Low,
    };

    let is_flagged = earliest
        .map(|d| matches!(d.year(), 2025 | 2026))
        .unwrap_or(false);

    let now = Utc::now().to_rfc3339();
    let earliest_str = earliest.map(|d| d.to_string());

    let evidence = serde_json::json!({
        "itunes_artist_id": resolution.as_ref().map(|r| r.itunes_artist_id),
        "matched_artist_name": resolution.as_ref().map(|r| r.matched_artist_name.clone()),
        "match_method": method.as_str(),
        "album_count": album_count,
        "earliest_release_date": earliest_str,
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
