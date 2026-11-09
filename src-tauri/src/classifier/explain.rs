//! Turns a stored verdict + its evidence JSON into plain-language lines for the tray menu, so
//! a listener can see *which* signals pushed an artist toward or away from a flag.
//!
//! Works off what was stored at classification time (verdicts are permanent, see
//! `classifier::classify`), so artists classified before this module existed get explained
//! too. Keys added later (`new_artist`, `musicbrainz_checked`) are preferred when present and
//! re-derived from the older keys when not.

use chrono::NaiveDate;
use serde_json::Value;

use super::{BURST_MAX_SPAN_DAYS, BURST_MIN_ALBUMS, LOW_CONFIDENCE_MAX, is_new_enough_to_flag};

/// Points toward "AI artist".
const AGAINST: &str = "✗";
/// Points away from it.
const FOR: &str = "✓";
/// Context only; didn't move the verdict.
const INFO: &str = "·";

pub fn explain(
    source: &str,
    evidence: &Value,
    is_flagged: bool,
    confidence: Option<f64>,
) -> Vec<String> {
    match source {
        "artist_name_denylist" => {
            let name = str_field(evidence, "denylisted_name").unwrap_or("an AI tool");
            vec![
                format!("{AGAINST} Credited to the AI music generator itself (\"{name}\")"),
                "= Flagged: AI tool credit".to_string(),
            ]
        }
        "itunes_search_api" => explain_itunes(evidence, is_flagged, confidence),
        other => vec![
            format!("{INFO} Checked by an unknown method ({other})"),
            verdict_line(is_flagged, confidence, &[]),
        ],
    }
}

fn explain_itunes(evidence: &Value, is_flagged: bool, confidence: Option<f64>) -> Vec<String> {
    let mut lines = Vec::new();

    let method = str_field(evidence, "match_method").unwrap_or("unresolved");
    let matched_name = str_field(evidence, "matched_artist_name");
    let unresolved = method == "unresolved";
    let album_count = evidence
        .get("album_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let earliest = str_field(evidence, "earliest_release_date")
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let burst = bool_field(evidence, "release_burst");
    let itunes_feat = bool_field(evidence, "itunes_feat_corroboration");
    let mb_found = bool_field(evidence, "musicbrainz_found");
    let mb_life_span = bool_field(evidence, "musicbrainz_life_span_corroboration");
    let mb_links = bool_field(evidence, "musicbrainz_external_links_corroboration");
    let invisible = bool_field(evidence, "invisible_everywhere");

    let new_artist = evidence
        .get("new_artist")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| earliest.is_some_and(is_new_enough_to_flag));
    let corroborated = itunes_feat || mb_life_span || mb_links;
    // Mirrors the gate in `classify`. `found` implies it was checked regardless.
    let mb_checked = evidence
        .get("musicbrainz_checked")
        .and_then(Value::as_bool)
        .unwrap_or(mb_found || (new_artist && !itunes_feat) || unresolved);

    // iTunes identity.
    lines.push(match (method, matched_name) {
        ("song_grounded", Some(name)) => {
            format!("{INFO} iTunes: found via this song (as \"{name}\")")
        }
        ("artist_name", Some(name)) => {
            format!("{INFO} iTunes: matched by name only (\"{name}\"), less certain")
        }
        _ => format!("{AGAINST} iTunes: no confident match"),
    });

    // Release history.
    if let Some(date) = earliest {
        lines.push(if new_artist {
            format!("{AGAINST} First release {date}, after the Mar 2024 AI-tools cutoff")
        } else {
            format!("{FOR} First release {date}, before AI tools matured (Mar 2024)")
        });
    } else if !unresolved {
        lines.push(format!("{INFO} No release dates on iTunes"));
    }

    if !unresolved {
        lines.push(if burst {
            format!(
                "{AGAINST} Release burst: {album_count} releases within {BURST_MAX_SPAN_DAYS} days"
            )
        } else if album_count as usize >= BURST_MIN_ALBUMS {
            format!("{FOR} No release burst ({album_count} releases, spread out)")
        } else {
            format!("{FOR} No release burst ({album_count} releases)")
        });
    }

    // Corroboration only matters for a new artist; mark its absence against them only then.
    if !unresolved {
        lines.push(if itunes_feat {
            format!("{FOR} iTunes: credited as a feature by another artist")
        } else {
            let mark = if new_artist { AGAINST } else { INFO };
            format!("{mark} iTunes: no other artist credits them as a feature")
        });
    }

    lines.push(if !mb_checked {
        format!("{INFO} MusicBrainz: not needed")
    } else if !mb_found {
        format!("{AGAINST} MusicBrainz: not found")
    } else {
        match (mb_life_span, mb_links) {
            (true, true) => format!("{FOR} MusicBrainz: found, with dates and external links"),
            (true, false) => format!("{FOR} MusicBrainz: found, with documented dates"),
            (false, true) => format!("{FOR} MusicBrainz: found, with external links"),
            (false, false) => format!("{AGAINST} MusicBrainz: found, but no dates or links"),
        }
    });

    let mut reasons = Vec::new();
    if new_artist && !corroborated {
        reasons.push("new artist, nothing corroborating them");
    }
    if burst {
        reasons.push("release burst");
    }
    if invisible {
        reasons.push("not found on iTunes or MusicBrainz");
    }
    lines.push(verdict_line(is_flagged, confidence, &reasons));
    lines
}

fn verdict_line(is_flagged: bool, confidence: Option<f64>, reasons: &[&str]) -> String {
    match (is_flagged, reasons.is_empty()) {
        (true, false) => format!("= Flagged: {}", reasons.join("; ")),
        // The stored verdict is permanent; an older app version's rules may have produced it.
        (true, true) => "= Flagged (by an earlier version's rules)".to_string(),
        (false, _) if confidence.is_some_and(|c| c <= LOW_CONFIDENCE_MAX) => {
            "= Not flagged, but the lookup was uncertain".to_string()
        }
        (false, _) => "= Not flagged".to_string(),
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn bool_field(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn itunes(e: Value, flagged: bool, conf: f64) -> Vec<String> {
        explain("itunes_search_api", &e, flagged, Some(conf))
    }

    #[test]
    fn new_uncorroborated_artist() {
        let lines = itunes(
            json!({
                "matched_artist_name": "Charli Morgan",
                "match_method": "song_grounded",
                "album_count": 3,
                "earliest_release_date": "2024-09-20",
                "release_burst": false,
                "itunes_feat_corroboration": false,
                "musicbrainz_found": true,
                "musicbrainz_life_span_corroboration": false,
                "musicbrainz_external_links_corroboration": false,
                "invisible_everywhere": false,
            }),
            true,
            1.0,
        );
        assert_eq!(
            lines,
            vec![
                "· iTunes: found via this song (as \"Charli Morgan\")",
                "✗ First release 2024-09-20, after the Mar 2024 AI-tools cutoff",
                "✓ No release burst (3 releases)",
                "✗ iTunes: no other artist credits them as a feature",
                "✗ MusicBrainz: found, but no dates or links",
                "= Flagged: new artist, nothing corroborating them",
            ]
        );
    }

    #[test]
    fn established_artist_skips_musicbrainz() {
        let lines = itunes(
            json!({
                "matched_artist_name": "Queen",
                "match_method": "song_grounded",
                "album_count": 40,
                "earliest_release_date": "1973-07-13",
                "release_burst": false,
                "itunes_feat_corroboration": false,
                "musicbrainz_found": false,
                "invisible_everywhere": false,
            }),
            false,
            1.0,
        );
        assert_eq!(
            lines,
            vec![
                "· iTunes: found via this song (as \"Queen\")",
                "✓ First release 1973-07-13, before AI tools matured (Mar 2024)",
                "✓ No release burst (40 releases, spread out)",
                "· iTunes: no other artist credits them as a feature",
                "· MusicBrainz: not needed",
                "= Not flagged",
            ]
        );
    }

    #[test]
    fn invisible_everywhere() {
        let lines = itunes(
            json!({
                "match_method": "unresolved",
                "album_count": 0,
                "musicbrainz_found": false,
                "invisible_everywhere": true,
            }),
            true,
            0.2,
        );
        assert_eq!(
            lines,
            vec![
                "✗ iTunes: no confident match",
                "✗ MusicBrainz: not found",
                "= Flagged: not found on iTunes or MusicBrainz",
            ]
        );
    }

    #[test]
    fn burst_and_corroborated_new_artist() {
        let lines = itunes(
            json!({
                "matched_artist_name": "X",
                "match_method": "artist_name",
                "album_count": 7,
                "earliest_release_date": "2025-01-01",
                "release_burst": true,
                "itunes_feat_corroboration": true,
                "invisible_everywhere": false,
                "new_artist": true,
                "musicbrainz_checked": false,
            }),
            true,
            0.6,
        );
        assert!(lines.contains(&"✗ Release burst: 7 releases within 30 days".to_string()));
        assert!(lines.contains(&"✓ iTunes: credited as a feature by another artist".to_string()));
        assert!(lines.contains(&"· MusicBrainz: not needed".to_string()));
        assert_eq!(lines.last().unwrap(), "= Flagged: release burst");
    }

    #[test]
    fn unresolved_but_on_musicbrainz_is_uncertain_not_flagged() {
        let lines = itunes(
            json!({
                "match_method": "unresolved",
                "musicbrainz_found": true,
                "musicbrainz_external_links_corroboration": true,
                "invisible_everywhere": false,
            }),
            false,
            0.2,
        );
        assert_eq!(
            lines,
            vec![
                "✗ iTunes: no confident match",
                "✓ MusicBrainz: found, with external links",
                "= Not flagged, but the lookup was uncertain",
            ]
        );
    }

    #[test]
    fn denylisted_tool_name() {
        let lines = explain(
            "artist_name_denylist",
            &json!({"denylisted_name": "Suno"}),
            true,
            Some(1.0),
        );
        assert_eq!(
            lines,
            vec![
                "✗ Credited to the AI music generator itself (\"Suno\")",
                "= Flagged: AI tool credit",
            ]
        );
    }

    #[test]
    fn flag_with_no_current_reason_is_attributed_to_older_rules() {
        let lines = itunes(
            json!({
                "matched_artist_name": "Y",
                "match_method": "song_grounded",
                "album_count": 2,
                "earliest_release_date": "2015-01-01",
                "invisible_everywhere": false,
            }),
            true,
            1.0,
        );
        assert_eq!(
            lines.last().unwrap(),
            "= Flagged (by an earlier version's rules)"
        );
    }
}
