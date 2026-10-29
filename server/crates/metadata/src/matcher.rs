//! Matching parsed titles to TMDB results.

use crate::{Result, tmdb::SearchResult};

/// Minimum score for an accepted match.
pub const THRESHOLD: f64 = 0.75;

/// What the scanner parsed from a file/folder name.
#[derive(Debug, Clone, Default)]
pub struct MatchQuery {
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
}

/// A scored-able candidate.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub tmdb_id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub popularity: f64,
}

impl From<&SearchResult> for Candidate {
    fn from(r: &SearchResult) -> Self {
        Candidate {
            tmdb_id: r.id,
            title: r.display_title().to_string(),
            original_title: r.original_display_title().map(str::to_string),
            year: r.year(),
            popularity: r.popularity.unwrap_or(0.0),
        }
    }
}

/// Lowercase, strip punctuation/articles-insensitive, collapse whitespace.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == '&' {
            out.push_str(" and ");
        } else if c == '\'' || c == '’' {
            // drop apostrophes: "Don't" -> "dont"
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Score in 0..=1: title similarity, adjusted by year agreement. Pure.
pub fn score(q: &MatchQuery, c: &Candidate) -> f64 {
    let nq = normalize(&q.title);
    let mut sim = strsim::normalized_damerau_levenshtein(&nq, &normalize(&c.title));
    if let Some(o) = &c.original_title {
        sim = sim.max(strsim::normalized_damerau_levenshtein(&nq, &normalize(o)));
    }
    let adj = match (q.year, c.year) {
        (Some(a), Some(b)) => match (a - b).abs() {
            0 => 0.1,
            1 => 0.05,
            _ => -0.3,
        },
        _ => 0.0,
    };
    (sim + adj).clamp(0.0, 1.0)
}

/// Pick the best candidate at/above [`THRESHOLD`]. An exact `tmdb_id` hint
/// short-circuits (returned even if not in `cands`, as id-only).
pub fn best_match<'a>(q: &MatchQuery, cands: &'a [Candidate]) -> Option<&'a Candidate> {
    let mut best: Option<(&Candidate, f64)> = None;
    for c in cands {
        let s = score(q, c);
        let better = match best {
            None => true,
            Some((b, bs)) => s > bs || (s == bs && c.popularity > b.popularity),
        };
        if better {
            best = Some((c, s));
        }
    }
    best.filter(|(_, s)| *s >= THRESHOLD).map(|(c, _)| c)
}

/// Resolve a query to a TMDB id using the client. `tv` selects TV search.
pub async fn resolve(client: &crate::TmdbClient, q: &MatchQuery, tv: bool) -> Result<Option<i64>> {
    if let Some(id) = q.tmdb_id {
        return Ok(Some(id));
    }
    let res = if tv {
        client.search_tv(&q.title, q.year).await?
    } else {
        client.search_movie(&q.title, q.year).await?
    };
    let cands: Vec<Candidate> = res.results.iter().map(Candidate::from).collect();
    Ok(best_match(q, &cands).map(|c| c.tmdb_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: i64, t: &str, y: Option<i32>) -> Candidate {
        Candidate {
            tmdb_id: id,
            title: t.into(),
            original_title: None,
            year: y,
            popularity: 1.0,
        }
    }
    fn q(t: &str, y: Option<i32>) -> MatchQuery {
        MatchQuery {
            title: t.into(),
            year: y,
            tmdb_id: None,
        }
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize("Don't Look-Up!"), "dont look up");
        assert_eq!(normalize("Tom & Jerry"), "tom and jerry");
    }

    #[test]
    fn exact_year_scores_full() {
        assert_eq!(
            score(
                &q("The Matrix", Some(1999)),
                &cand(1, "The Matrix", Some(1999))
            ),
            1.0
        );
    }

    #[test]
    fn year_off_by_one_ok_off_by_many_rejected() {
        let c = cand(1, "Dune", Some(2021));
        assert!(score(&q("Dune", Some(2022)), &c) >= THRESHOLD);
        assert!(score(&q("Dune", Some(1984)), &c) < THRESHOLD);
    }

    #[test]
    fn picks_best_and_rejects_poor() {
        let cands = vec![
            cand(1, "Dune", Some(1984)),
            cand(2, "Dune", Some(2021)),
            cand(3, "Dune: Part Two", Some(2024)),
        ];
        assert_eq!(
            best_match(&q("Dune", Some(2021)), &cands).unwrap().tmdb_id,
            2
        );
        assert!(best_match(&q("Completely Different Thing", None), &cands).is_none());
        assert!(best_match(&q("x", None), &[]).is_none());
    }

    #[test]
    fn uses_original_title() {
        let mut c = cand(1, "Spirited Away", Some(2001));
        c.original_title = Some("千と千尋の神隠し".into());
        assert!(score(&q("千と千尋の神隠し", Some(2001)), &c) >= THRESHOLD);
    }
}
