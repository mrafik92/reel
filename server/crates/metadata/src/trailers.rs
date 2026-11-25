//! Library trailers (#302): the YouTube id of a movie's or show's main trailer, from TMDB.
//!
//! Enrichment stores it with the rest of the metadata ([`store`]). Titles enriched before
//! trailers existed get theirs from a slow background backfill ([`spawn`]) that asks TMDB's
//! `/videos` once per matched title and records the attempt in `trailer_checked_at`, so a title
//! without a trailer is asked again only after [`RECHECK_DAYS`].
//!
//! A `"trailer"` entry in an item's `locked_fields_json` keeps its stored trailer as it is.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::{Error, Result, TmdbClient, tmdb::is_youtube_id};

/// Titles without a trailer are asked again after this many days (trailers appear before a
/// release, and TMDB editors add them late for older titles).
pub const RECHECK_DAYS: i64 = 30;

/// Record a TMDB answer for `item_id`: the trailer when there is one, and the time of the check.
/// A missing trailer never clears one already stored, and a locked trailer is left alone.
pub async fn store(pool: &SqlitePool, item_id: i64, youtube_id: Option<&str>) -> Result<()> {
    let youtube_id = youtube_id.filter(|k| is_youtube_id(k));
    sqlx::query(
        "UPDATE media_items SET trailer_checked_at = unixepoch(),
           trailer_youtube_id = CASE
             WHEN EXISTS (SELECT 1 FROM json_each(locked_fields_json)
                          WHERE lower(json_each.value) = 'trailer') THEN trailer_youtube_id
             ELSE COALESCE(?, trailer_youtube_id) END
         WHERE id = ?",
    )
    .bind(youtube_id)
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Run the backfill forever: one title per second while there is work, then a check every 10
/// minutes (new matches from scans are covered by enrichment itself).
pub fn spawn(pool: SqlitePool, tmdb: TmdbClient) {
    tokio::spawn(async move {
        loop {
            let delay = match backfill_one(&pool, &tmdb).await {
                Ok(true) => Duration::from_secs(1),
                Ok(false) => Duration::from_secs(600),
                Err(e) => {
                    tracing::debug!(error = %e, "trailer backfill will retry");
                    Duration::from_secs(60)
                }
            };
            tokio::time::sleep(delay).await;
        }
    });
}

/// Ask TMDB for one matched movie or show that has no trailer and wasn't checked recently.
/// `Ok(false)` when there is nothing to do.
pub async fn backfill_one(pool: &SqlitePool, tmdb: &TmdbClient) -> Result<bool> {
    let Some((id, kind, tmdb_id)) = sqlx::query_as::<_, (i64, String, i64)>(
        "SELECT id, kind, tmdb_id FROM media_items
         WHERE kind IN ('movie', 'show') AND tmdb_id IS NOT NULL AND trailer_youtube_id IS NULL
           AND (trailer_checked_at IS NULL OR trailer_checked_at < unixepoch() - ? * 86400)
         ORDER BY trailer_checked_at, id LIMIT 1",
    )
    .bind(RECHECK_DAYS)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(false);
    };
    let trailer = match tmdb.videos(kind == "show", tmdb_id).await {
        Ok(v) => v.youtube_trailer(),
        // The title is gone from TMDB: record the check, try again in a month.
        Err(Error::Status(404)) => None,
        Err(e) => return Err(e),
    };
    store(pool, id, trailer.as_deref()).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    async fn db() -> SqlitePool {
        let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1, 'L', 'movies')")
            .execute(&db)
            .await
            .unwrap();
        db
    }

    async fn item(db: &SqlitePool, id: i64, kind: &str, tmdb: Option<i64>, locked: &str) {
        sqlx::query(
            "INSERT INTO media_items (id, library_id, kind, title, tmdb_id, locked_fields_json)
             VALUES (?, 1, ?, 'T', ?, ?)",
        )
        .bind(id)
        .bind(kind)
        .bind(tmdb)
        .bind(locked)
        .execute(db)
        .await
        .unwrap();
    }

    async fn trailer(db: &SqlitePool, id: i64) -> (Option<String>, bool) {
        sqlx::query_as(
            "SELECT trailer_youtube_id, trailer_checked_at IS NOT NULL FROM media_items WHERE id = ?",
        )
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn store_keeps_existing_and_locked_trailers_and_rejects_bad_ids() {
        let db = db().await;
        item(&db, 1, "movie", Some(10), "[]").await;
        item(&db, 2, "movie", Some(11), r#"["Trailer"]"#).await;

        store(&db, 1, Some("aqz-KE-bpKQ")).await.unwrap();
        store(&db, 1, None).await.unwrap();
        assert_eq!(trailer(&db, 1).await, (Some("aqz-KE-bpKQ".into()), true));
        store(&db, 1, Some("<script>")).await.unwrap();
        assert_eq!(trailer(&db, 1).await.0.as_deref(), Some("aqz-KE-bpKQ"));

        store(&db, 2, Some("aqz-KE-bpKQ")).await.unwrap();
        assert_eq!(trailer(&db, 2).await, (None, true));
    }

    #[tokio::test]
    async fn backfill_asks_once_per_matched_title() {
        let db = db().await;
        item(&db, 1, "movie", Some(10), "[]").await;
        item(&db, 2, "show", Some(20), "[]").await;
        item(&db, 3, "movie", None, "[]").await; // unmatched: never asked
        item(&db, 4, "season", Some(20), "[]").await; // not a title

        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/10/videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[
                {"key":"aqz-KE-bpKQ","site":"YouTube","type":"Trailer","official":true}
            ]})))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/tv/20/videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[]})))
            .expect(1)
            .mount(&s)
            .await;
        let tmdb = TmdbClient::with_base_url("k", s.uri());
        assert!(backfill_one(&db, &tmdb).await.unwrap());
        assert!(backfill_one(&db, &tmdb).await.unwrap());
        assert!(!backfill_one(&db, &tmdb).await.unwrap());

        assert_eq!(trailer(&db, 1).await, (Some("aqz-KE-bpKQ".into()), true));
        assert_eq!(trailer(&db, 2).await, (None, true));
        assert_eq!(trailer(&db, 3).await, (None, false));
    }
}
