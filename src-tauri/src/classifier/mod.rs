use chrono::{NaiveDate, Utc};

use crate::db::Db;
use crate::db::artist_classification::NewClassification;
use crate::db::classification_evidence::NewEvidence;
use crate::itunes::search::MatchMethod;
use crate::itunes::{ItunesClient, albums, search};
use crate::musicbrainz::{self, MusicBrainzClient};
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

/// AI music tools capable of passing as human only matured from roughly Suno v3 (March 2024)
/// / Udio (April 2024) onward -- a placeholder as rough as the rest of this heuristic stack,
/// not a rigorously sourced threshold, but a fixed historical floor that doesn't need updating
/// as further years pass, unlike an enumerated year list (`year() in {2025, 2026}` needing
/// "2027" added by hand next year).
///
/// This used to be paired with a trailing window measured from `today` ("debuted within the
/// last 24 months"), on the theory that a real artist active for years without corroboration
/// should eventually stop looking suspicious. That reasoning assumed classification happens
/// continuously, re-checking artists as time passes. It doesn't: `classify` runs exactly once
/// per artist, at whatever moment this listener's library first plays them (media/mod.rs,
/// gated on `get_classification` returning `None`). So "today" was really "whenever this
/// listener happened to press play" -- unrelated to the artist's actual debut-to-now gap, and
/// identical to whatever "now" the live iTunes/MusicBrainz lookups below already reflect. Worse,
/// aging out of that window also skipped the MusicBrainz corroboration attempt (gated on
/// `new_artist`), so a late first play was a free pass on the one real verification step too.
/// The floor alone is timeless and doesn't depend on when a listener happens to press play, so
/// it's the only bound worth keeping; corroboration/burst/invisible-everywhere carry the rest.
fn ai_capability_floor() -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 3, 1).expect("valid hardcoded date")
}

/// True if `earliest` falls after the AI-capability floor. Confirmed live this catches real
/// cases the old year()-based check missed entirely (e.g. "END EVE", earliest 2024-09-20, a
/// confirmed AI artist that the old 2025/2026-only check let straight through).
fn is_new_enough_to_flag(earliest: NaiveDate) -> bool {
    earliest >= ai_capability_floor()
}

/// Title markers for the AI-cover-mashup convention observed across multiple confirmed-AI
/// artists (e.g. "Hall of Fame (feat. Seraphina Rossi) [Female Version]", "Smack That - Female
/// Version"). Discovered live: a farm can trivially manufacture "third-party corroboration" by
/// cross-crediting its own synthetic personas as `(feat. ...)` on each other's tracks under this
/// exact naming convention -- confirmed in practice via "Eiden Xii", whose own earliest release
/// predates the flag window (an old, apparently legitimate release) but who now also credits
/// several other AI personas as "feat." across dozens of Female/Rock Version singles. A credit
/// under this convention doesn't count as real corroboration, regardless of how clean the
/// crediting primary artist otherwise looks.
const AI_COVER_MASHUP_TITLE_MARKERS: &[&str] = &[
    "female version",
    "rock version",
    "but it hits hard",
    "but it hits different",
];

fn looks_like_ai_cover_mashup_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    AI_COVER_MASHUP_TITLE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// True if any of the artist's iTunes collections is credited to a *different* primary artist
/// name under a title that doesn't match the AI-cover-mashup convention -- i.e. this artist
/// shows up as a featured collaborator on someone else's ordinarily-titled release (e.g.
/// "Fliegen (feat. Tiziano Guerzoni)" credited to "G Kollektiv"). A separate, real catalog
/// choosing to credit them like this is much harder to fake cheaply than self-published volume,
/// so it's treated as strong evidence against "this is a synthetic persona" -- unless the title
/// itself is the tell (see `looks_like_ai_cover_mashup_title`).
fn has_third_party_corroboration(artist_name: &str, entries: &[albums::AlbumEntry]) -> bool {
    let normalized = normalize_artist_name(artist_name);
    entries.iter().any(|e| {
        let different_primary_artist = e
            .artist_name
            .as_deref()
            .is_some_and(|name| normalize_artist_name(name) != normalized);
        let mashup_titled = e
            .collection_name
            .as_deref()
            .is_some_and(looks_like_ai_cover_mashup_title);
        different_primary_artist && !mashup_titled
    })
}

/// No confident match on iTunes *and* no match at all on MusicBrainz. A real artist, however
/// obscure or new, is rarely invisible to both a commercial catalog and a community-edited one
/// at once -- accepted trade-off: this also catches a genuinely new, tiny, real independent
/// artist who hasn't been catalogued anywhere yet.
fn is_invisible_everywhere(method: MatchMethod, mb_found: bool) -> bool {
    matches!(method, MatchMethod::Unresolved) && !mb_found
}

/// The full §3 pipeline: resolve -> fetch albums -> earliest-date -> confidence -> dual-table
/// write. Never skips the write, even when unresolved or zero-album (docs/phase0-plan.md §3.1
/// step 5) -- every artist encountered gets exactly one permanent row.
pub async fn classify(
    db: &Db,
    itunes: &ItunesClient,
    musicbrainz: &MusicBrainzClient,
    req: ClassifyRequest,
) -> anyhow::Result<bool> {
    if is_ai_tool_denylisted(&req.artist_name) {
        log::info!(
            "scoring '{}': denylisted AI-tool name => is_flagged=true",
            req.artist_name
        );
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

    let (earliest, album_count, burst, itunes_corroborated) = match &resolution {
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
    let new_artist = earliest.is_some_and(is_new_enough_to_flag);
    let itunes_unresolved = matches!(method, MatchMethod::Unresolved);

    // Worth the extra MusicBrainz round-trip in two cases: iTunes would otherwise flag on
    // recency alone and hasn't already corroborated it, or iTunes couldn't resolve the artist
    // at all (checked here for the invisible-everywhere signal below). Keeps the one-time cost
    // paid only where it can change the verdict.
    let mb = if (new_artist && !itunes_corroborated) || itunes_unresolved {
        match musicbrainz::lookup_corroboration(musicbrainz, &req.artist_name, &req.track_title)
            .await
        {
            Ok(mb) => mb,
            Err(err) => {
                log::warn!(
                    "MusicBrainz lookup failed for '{}': {err:?}",
                    req.artist_name
                );
                musicbrainz::MbCorroboration::default()
            }
        }
    } else {
        musicbrainz::MbCorroboration::default()
    };

    let corroborated = itunes_corroborated || mb.any();
    let invisible_everywhere = is_invisible_everywhere(method, mb.found);
    let is_flagged = (new_artist && !corroborated) || burst || invisible_everywhere;

    log::info!(
        "scoring '{}': method={} confidence={:?} earliest={} album_count={} new_artist={} burst={} itunes_corroborated={} mb_found={} mb_life_span={} mb_external_links={} invisible_everywhere={} => is_flagged={}",
        req.artist_name,
        method.as_str(),
        confidence,
        earliest.map(|d| d.to_string()).as_deref().unwrap_or("none"),
        album_count,
        new_artist,
        burst,
        itunes_corroborated,
        mb.found,
        mb.has_life_span,
        mb.has_external_links,
        invisible_everywhere,
        is_flagged,
    );

    let now = Utc::now().to_rfc3339();
    let earliest_str = earliest.map(|d| d.to_string());

    let evidence = serde_json::json!({
        "itunes_artist_id": resolution.as_ref().map(|r| r.itunes_artist_id),
        "matched_artist_name": resolution.as_ref().map(|r| r.matched_artist_name.clone()),
        "match_method": method.as_str(),
        "album_count": album_count,
        "earliest_release_date": earliest_str,
        "release_burst": burst,
        "itunes_feat_corroboration": itunes_corroborated,
        "musicbrainz_found": mb.found,
        "musicbrainz_life_span_corroboration": mb.has_life_span,
        "musicbrainz_external_links_corroboration": mb.has_external_links,
        "invisible_everywhere": invisible_everywhere,
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

    #[test]
    fn end_eve_case_is_caught_by_the_floor_check() {
        // Confirmed live: "END EVE" (earliest 2024-09-20) is a real AI artist that the old
        // year()-in-{2025,2026} check let through entirely. The floor check must catch it.
        assert!(is_new_enough_to_flag(date("2024-09-20")));
    }

    #[test]
    fn before_ai_capability_floor_is_never_new_enough() {
        assert!(!is_new_enough_to_flag(date("2024-02-29")));
    }

    #[test]
    fn does_not_age_out_no_matter_how_long_ago_it_debuted() {
        // Unlike the old trailing-window design, a debut just after the floor stays "new
        // enough to flag" forever -- there's no re-evaluation to age it out of, since
        // classification runs exactly once per artist at first play.
        assert!(is_new_enough_to_flag(date("2024-09-20")));
    }

    fn entry(release_date: &str, artist_name: Option<&str>) -> albums::AlbumEntry {
        entry_titled(release_date, artist_name, "Some Song - Single")
    }

    fn entry_titled(
        release_date: &str,
        artist_name: Option<&str>,
        collection_name: &str,
    ) -> albums::AlbumEntry {
        albums::AlbumEntry {
            release_date: date(release_date),
            artist_name: artist_name.map(str::to_string),
            collection_name: Some(collection_name.to_string()),
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

    #[test]
    fn not_corroborated_when_feat_credit_is_an_ai_cover_mashup_title() {
        // The exact Seraphina Rossi / "Eiden Xii" case found live: a "feat." credit exists,
        // but every one of them is titled with the AI-cover-mashup convention -- including the
        // "(But It Hits Hard/Different)" variant that a first, narrower marker list missed.
        let entries = vec![
            entry_titled(
                "2025-11-14",
                Some("Eiden Xii"),
                "Dark Horse (feat. Seraphina Rossi) [Rock Version] - Single",
            ),
            entry_titled(
                "2026-01-16",
                Some("Eiden Xii"),
                "Hall of Fame (feat. Seraphina Rossi) [Female Version] - Single",
            ),
            entry_titled(
                "2025-12-12",
                Some("Eiden Xii"),
                "Lovely (But It Hits Hard) [feat. Seraphina Rossi] - Single",
            ),
            entry_titled(
                "2025-11-14",
                Some("Angelina De Luca"),
                "Little Do You Know (But it hits different) (feat. Seraphina Rossi) - Single",
            ),
        ];
        assert!(!has_third_party_corroboration("Seraphina Rossi", &entries));
    }

    #[test]
    fn corroborated_when_feat_credit_is_an_ordinary_title() {
        let entries = vec![entry_titled(
            "2026-05-01",
            Some("G Kollektiv"),
            "Fliegen (feat. Tiziano Guerzoni) - Single",
        )];
        assert!(has_third_party_corroboration("Tiziano Guerzoni", &entries));
    }

    #[test]
    fn looks_like_ai_cover_mashup_title_matches_known_markers_case_insensitively() {
        assert!(looks_like_ai_cover_mashup_title(
            "Smack That - Female Version"
        ));
        assert!(looks_like_ai_cover_mashup_title(
            "Hot N Cold (feat. X) [ROCK VERSION] - Single"
        ));
        assert!(!looks_like_ai_cover_mashup_title("Fliegen - Single"));
    }

    #[test]
    fn mb_corroboration_any_is_true_if_either_signal_present() {
        assert!(!musicbrainz::MbCorroboration::default().any());
        assert!(
            musicbrainz::MbCorroboration {
                found: true,
                has_life_span: true,
                has_external_links: false,
            }
            .any()
        );
        assert!(
            musicbrainz::MbCorroboration {
                found: true,
                has_life_span: false,
                has_external_links: true,
            }
            .any()
        );
    }

    #[test]
    fn invisible_everywhere_requires_unresolved_and_no_musicbrainz_match() {
        assert!(is_invisible_everywhere(MatchMethod::Unresolved, false));
        assert!(!is_invisible_everywhere(MatchMethod::Unresolved, true));
        assert!(!is_invisible_everywhere(MatchMethod::SongGrounded, false));
        assert!(!is_invisible_everywhere(MatchMethod::ArtistName, false));
    }
}
