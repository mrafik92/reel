//! Local-only enrichment: `.nfo` + artwork with no TMDB client at all.
use reel_metadata::Enricher;
use sqlx::SqlitePool;

async fn pool() -> SqlitePool {
    let p = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO libraries (id, name, kind) VALUES (1, 'L', 'movies'), (2, 'T', 'shows')",
    )
    .execute(&p)
    .await
    .unwrap();
    p
}

async fn file(p: &SqlitePool, item: i64, path: &std::path::Path) -> i64 {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"x").unwrap();
    sqlx::query_scalar(
        "INSERT INTO media_files (item_id, path, size, mtime) VALUES (?, ?, 1, 1) RETURNING id",
    )
    .bind(item)
    .bind(path.to_str().unwrap())
    .fetch_one(p)
    .await
    .unwrap()
}

type EpRow = (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

async fn row(p: &SqlitePool, id: i64) -> EpRow {
    sqlx::query_as(
        "SELECT title, overview, air_date, runtime_s, poster FROM media_items WHERE id = ?",
    )
    .bind(id)
    .fetch_one(p)
    .await
    .unwrap()
}

#[tokio::test]
async fn movie_nfo_and_art_without_a_tmdb_key() {
    let e = Enricher::local();
    assert!(!e.has_tmdb() && e.client().is_none());
    let dir = tempfile::tempdir().unwrap();
    let p = pool().await;
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, year, locked_fields_json) VALUES (1,1,'movie','dune',2021,'[\"title\"]')")
        .execute(&p)
        .await
        .unwrap();
    file(&p, 1, &dir.path().join("Dune (2021)/Dune (2021).mkv")).await;
    std::fs::write(
        dir.path().join("Dune (2021)/Dune (2021)-poster.jpg"),
        b"img",
    )
    .unwrap();
    std::fs::write(dir.path().join("Dune (2021)/fanart.jpg"), b"img").unwrap();
    std::fs::write(
        dir.path().join("Dune (2021)/Dune (2021).nfo"),
        r#"<movie><title>Dune: Part One</title><year>2021</year><plot>Local plot</plot><runtime>155</runtime>
           <mpaa>Rated PG-13</mpaa><genre>Sci-Fi</genre><genre>Adventure</genre>
           <ratings><rating name="x"><value>7.8</value></rating></ratings>
           <uniqueid type="tmdb">438631</uniqueid><uniqueid type="imdb">tt1160419</uniqueid>
           <actor><name>Zendaya</name><role>Chani</role></actor><actor><name>Timothée</name><role>Paul</role></actor></movie>"#,
    )
    .unwrap();

    e.enrich(&p, 1).await.unwrap();
    let (title, overview, rating, mpaa, rt, tmdb, imdb, poster, back): (String, String, f64, String, i64, i64, String, String, String) =
        sqlx::query_as("SELECT title, overview, rating, content_rating, runtime_s, tmdb_id, imdb_id, poster, backdrop FROM media_items WHERE id = 1")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(title, "dune", "locked field untouched");
    assert_eq!(
        (
            overview.as_str(),
            rating,
            mpaa.as_str(),
            rt,
            tmdb,
            imdb.as_str()
        ),
        ("Local plot", 7.8, "PG-13", 155 * 60, 438631, "tt1160419")
    );
    assert!(
        poster.starts_with("file://") && poster.ends_with("Dune (2021)-poster.jpg"),
        "{poster}"
    );
    assert!(back.ends_with("fanart.jpg"));
    let genres: Vec<String> = sqlx::query_scalar("SELECT g.name FROM item_genres ig JOIN genres g ON g.id = ig.genre_id WHERE ig.item_id = 1 ORDER BY g.name")
        .fetch_all(&p)
        .await
        .unwrap();
    assert_eq!(genres, ["Adventure", "Sci-Fi"]);
    let cast: Vec<(String, String)> = sqlx::query_as("SELECT p.name, c.character FROM credits c JOIN people p ON p.id = c.person_id WHERE c.item_id = 1 ORDER BY c.ord")
        .fetch_all(&p)
        .await
        .unwrap();
    assert_eq!(
        cast,
        [
            ("Zendaya".to_string(), "Chani".to_string()),
            ("Timothée".to_string(), "Paul".to_string())
        ]
    );

    // idempotent: no duplicate people/credits
    e.enrich(&p, 1).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM people")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(n, 2);
    // an item without any sidecar is left alone (and not an error)
    sqlx::query(
        "INSERT INTO media_items (id, library_id, kind, title) VALUES (2,1,'movie','Bare')",
    )
    .execute(&p)
    .await
    .unwrap();
    file(&p, 2, &dir.path().join("Bare/Bare.mkv")).await;
    e.enrich(&p, 2).await.unwrap();
    let t: String = sqlx::query_scalar("SELECT title FROM media_items WHERE id = 2")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(t, "Bare");
}

#[tokio::test]
async fn show_season_and_episode_nfos_including_multi_episode_files() {
    let e = Enricher::local();
    let dir = tempfile::tempdir().unwrap();
    let show = dir.path().join("Duo (2020)");
    let s1 = show.join("Season 01");
    let p = pool().await;
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title) VALUES (10,2,'show','Duo')")
        .execute(&p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no) VALUES (11,2,'season',10,'Season 1',1)").execute(&p).await.unwrap();
    for (id, no) in [(12, 1), (13, 2), (14, 3)] {
        sqlx::query("INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no, episode_no) VALUES (?,2,'episode',11,?,1,?)")
            .bind(id)
            .bind(format!("Episode {no}"))
            .bind(no)
            .execute(&p)
            .await
            .unwrap();
    }
    file(&p, 12, &s1.join("Duo - S01E01.mkv")).await;
    let f2 = file(&p, 13, &s1.join("Duo - S01E02E03.mkv")).await;
    sqlx::query("INSERT INTO media_file_links (file_id, item_id) VALUES (?, 14)")
        .bind(f2)
        .execute(&p)
        .await
        .unwrap();

    std::fs::write(show.join("tvshow.nfo"), "<tvshow><title>Duo Show</title><plot>Show plot</plot><genre>Drama</genre><uniqueid type=\"tmdb\">777</uniqueid></tvshow>").unwrap();
    std::fs::write(show.join("poster.jpg"), b"i").unwrap();
    std::fs::write(show.join("season01-poster.jpg"), b"i").unwrap();
    std::fs::write(
        s1.join("Duo - S01E01.nfo"),
        "<episodedetails><title>Pilot</title><plot>First</plot><aired>2020-01-01</aired><runtime>45</runtime><season>1</season><episode>1</episode></episodedetails>",
    )
    .unwrap();
    std::fs::write(s1.join("Duo - S01E01-thumb.jpg"), b"i").unwrap();
    // one nfo, two blocks for the double episode
    std::fs::write(
        s1.join("Duo - S01E02E03.nfo"),
        "<episodedetails><title>Two</title><season>1</season><episode>2</episode><plot>P2</plot></episodedetails>\n\
         <episodedetails><title>Three</title><season>1</season><episode>3</episode><plot>P3</plot><aired>2020-01-15</aired></episodedetails>",
    )
    .unwrap();

    e.enrich(&p, 10).await.unwrap();
    let (t, o, tm): (String, String, i64) =
        sqlx::query_as("SELECT title, overview, tmdb_id FROM media_items WHERE id = 10")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!((t.as_str(), o.as_str(), tm), ("Duo Show", "Show plot", 777));
    let poster: String = sqlx::query_scalar("SELECT poster FROM media_items WHERE id = 10")
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(poster.ends_with("poster.jpg"));
    let sp: String = sqlx::query_scalar("SELECT poster FROM media_items WHERE id = 11")
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(sp.ends_with("season01-poster.jpg"), "{sp}");
    let e1 = row(&p, 12).await;
    assert_eq!(
        (e1.0.as_str(), e1.1.as_deref(), e1.2.as_deref(), e1.3),
        ("Pilot", Some("First"), Some("2020-01-01"), Some(45 * 60))
    );
    assert!(e1.4.unwrap().ends_with("S01E01-thumb.jpg"));
    let e2 = row(&p, 13).await;
    let e3 = row(&p, 14).await;
    assert_eq!((e2.0.as_str(), e2.1.as_deref()), ("Two", Some("P2")));
    assert_eq!(
        (e3.0.as_str(), e3.1.as_deref(), e3.2.as_deref()),
        ("Three", Some("P3"), Some("2020-01-15"))
    );
    let g: Vec<String> = sqlx::query_scalar("SELECT g.name FROM item_genres ig JOIN genres g ON g.id = ig.genre_id WHERE ig.item_id = 10").fetch_all(&p).await.unwrap();
    assert_eq!(g, ["Drama"]);
    // enriching via an episode id routes to the show
    e.enrich(&p, 14).await.unwrap();
}
