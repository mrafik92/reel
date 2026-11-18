//! Fill missing IPTV movie genres through TMDB without using a provider connection.

use std::time::Duration;

use reel_metadata::{TmdbClient, matcher::normalize, tmdb::SearchResult};
use sqlx::SqlitePool;

type Movie = (i64, String, Option<i32>, Option<i64>);

pub fn spawn(db: SqlitePool, tmdb: TmdbClient) {
    tokio::spawn(async move {
        loop {
            let delay = match backfill_one(&db, &tmdb).await {
                Ok(true) => Duration::from_secs(1),
                Ok(false) => Duration::from_secs(600),
                Err(_) => {
                    tracing::debug!("IPTV movie genre backfill will retry");
                    Duration::from_secs(60)
                }
            };
            tokio::time::sleep(delay).await;
        }
    });
}

async fn backfill_one(db: &SqlitePool, tmdb: &TmdbClient) -> anyhow::Result<bool> {
    let Some((id, title, year, existing_id)) = sqlx::query_as::<_, Movie>(
        "SELECT id, title, year, tmdb_id FROM vod_movies
         WHERE missing = 0 AND COALESCE(genres, '') = ''
           AND (genre_checked_at IS NULL OR genre_checked_at < unixepoch() - 30 * 86400)
         ORDER BY genre_checked_at, id LIMIT 1",
    )
    .fetch_optional(db)
    .await?
    else {
        return Ok(false);
    };

    let mut matched_id = existing_id;
    if existing_id.is_none() && !normalize(&title).is_empty() {
        let results = tmdb.search_movie(&title, year).await?;
        // If results span several pages, an unseen exact match could make this ambiguous.
        if results.total_pages <= 1 {
            matched_id = exact_match(&title, year, &results.results);
        }
    }

    let mut genres = None;
    if let Some(tmdb_id) = matched_id {
        match tmdb.movie_cached(tmdb_id).await {
            Ok(movie) if !movie.genres.is_empty() => {
                genres = Some(
                    movie
                        .genres
                        .iter()
                        .map(|g| g.name.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            Ok(_) | Err(reel_metadata::Error::Status(404)) => {}
            Err(error) => return Err(error.into()),
        }
    }

    sqlx::query(
        "UPDATE vod_movies SET genres = ?, tmdb_id = COALESCE(tmdb_id, ?),
         genre_checked_at = unixepoch() WHERE id = ? AND missing = 0
         AND COALESCE(genres, '') = '' AND title = ? AND year IS ? AND tmdb_id IS ?",
    )
    .bind(&genres)
    .bind(matched_id.filter(|_| genres.is_some()))
    .bind(id)
    .bind(title)
    .bind(year)
    .bind(existing_id)
    .execute(db)
    .await?;
    Ok(true)
}

fn exact_match(title: &str, year: Option<i32>, results: &[SearchResult]) -> Option<i64> {
    let title = normalize(title);
    if title.is_empty() {
        return None;
    }
    let mut matches = results.iter().filter(|result| {
        result
            .year()
            .is_some_and(|candidate_year| year.is_none_or(|year| candidate_year == year))
            && (normalize(result.display_title()) == title
                || result
                    .original_display_title()
                    .is_some_and(|name| normalize(name) == title))
    });
    let first = matches.next()?;
    matches
        .all(|result| result.id == first.id)
        .then_some(first.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: i64, title: &str, date: &str) -> SearchResult {
        SearchResult {
            id,
            title: Some(title.into()),
            release_date: Some(date.into()),
            ..Default::default()
        }
    }

    #[test]
    fn only_accepts_unambiguous_matches_from_the_same_year() {
        let hits = [
            hit(1, "The Thing", "1982-06-25"),
            hit(2, "The Thing", "2011-10-14"),
        ];
        assert_eq!(exact_match("The Thing!", Some(1982), &hits), Some(1));
        assert_eq!(exact_match("Thing", Some(1982), &hits), None);
        assert_eq!(exact_match("The Thing", Some(1983), &hits), None);
        assert_eq!(exact_match("The Thing", None, &hits), None);
        assert_eq!(exact_match("The Thing", None, &hits[..1]), Some(1));
        let ambiguous = [hits[0].clone(), hit(3, "The Thing", "1982-01-01")];
        assert_eq!(exact_match("The Thing", Some(1982), &ambiguous), None);
    }

    #[tokio::test]
    async fn backfill_persists_genres_and_skips_completed_movies() {
        use serde_json::json;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO iptv_sources (id, name, kind, url) VALUES (1, 'Panel', 'xtream', 'http://example.test')")
            .execute(&db).await.unwrap();
        sqlx::query("INSERT INTO vod_movies (source_id, stream_id, title, title_norm, year) VALUES (1, 1, 'The Thing', 'the thing', 1982)")
            .execute(&db).await.unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/search/movie"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_pages": 1, "results": [{"id": 1091, "title": "The Thing", "release_date": "1982-06-25"}]
            }))).expect(1).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/movie/1091"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 1091, "title": "The Thing", "genres": [{"id": 27, "name": "Horror"}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let tmdb = TmdbClient::with_base_url("test", server.uri());
        assert!(backfill_one(&db, &tmdb).await.unwrap());
        assert!(!backfill_one(&db, &tmdb).await.unwrap());

        let (genres, tmdb_id, checked): (String, i64, bool) =
            sqlx::query_as("SELECT genres, tmdb_id, genre_checked_at IS NOT NULL FROM vod_movies")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!((genres.as_str(), tmdb_id, checked), ("Horror", 1091, true));
    }
}
