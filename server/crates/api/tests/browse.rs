mod common;

use std::collections::HashSet;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{Limits, NewUser, Role};
use reel_metadata::TmdbClient;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let s = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

struct Env {
    state: AppState,
    admin: Router,
    db: SqlitePool,
    _dir: tempfile::TempDir,
}

async fn env(tmdb: Option<&MockServer>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    let mut state = AppState::new(db.clone(), cfg);
    if let Some(t) = tmdb {
        state = state.with_tmdb(TmdbClient::with_base_url("k", t.uri()));
    }
    let admin = common::authed(&state, app(state.clone())).await;
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'Movies','movies')")
        .execute(&db)
        .await
        .unwrap();
    Env {
        state,
        admin,
        db,
        _dir: dir,
    }
}

#[allow(clippy::too_many_arguments)]
async fn movie(
    db: &SqlitePool,
    id: i64,
    title: &str,
    year: i64,
    rating: Option<f64>,
    minutes: i64,
    genres: &[&str],
    tmdb: Option<i64>,
) {
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, sort_title, year, rating, runtime_s, tmdb_id, poster, added_at) VALUES (?,1,'movie',?,lower(?),?,?,?,?,'/p',?)")
        .bind(id).bind(title).bind(title).bind(year).bind(rating).bind(minutes * 60).bind(tmdb).bind(1_000_000 + id)
        .execute(db).await.unwrap();
    for g in genres {
        sqlx::query("INSERT OR IGNORE INTO genres (name) VALUES (?)")
            .bind(g)
            .execute(db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO item_genres (item_id, genre_id) SELECT ?, id FROM genres WHERE name = ?",
        )
        .bind(id)
        .bind(g)
        .execute(db)
        .await
        .unwrap();
    }
}

async fn watched(db: &SqlitePool, user: i64, item: i64, last: i64) {
    sqlx::query("INSERT INTO watch_state (user_id, item_id, position_s, played, play_count, last_played_at) VALUES (?,?,0,1,1,?)")
        .bind(user).bind(item).bind(last).execute(db).await.unwrap();
}

fn ids(v: &Value) -> Vec<i64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_i64().unwrap())
        .collect()
}

async fn seed_small(e: &Env) {
    let db = &e.db;
    movie(
        db,
        1,
        "Alpha Thriller",
        1995,
        Some(8.5),
        120,
        &["Thriller", "Crime"],
        Some(101),
    )
    .await;
    movie(
        db,
        2,
        "Bravo Comedy",
        2005,
        Some(6.0),
        90,
        &["Comedy"],
        Some(102),
    )
    .await;
    movie(
        db,
        3,
        "Charlie Thriller Short",
        2015,
        Some(7.2),
        85,
        &["Thriller"],
        Some(103),
    )
    .await;
    movie(
        db,
        4,
        "Delta Crime",
        2020,
        None,
        150,
        &["Crime", "Drama"],
        Some(104),
    )
    .await;
    movie(
        db,
        5,
        "Echo Drama",
        2021,
        Some(9.1),
        100,
        &["Drama"],
        Some(105),
    )
    .await;
    watched(db, 1, 1, 1_700_000_000).await; // admin (user 1) saw Alpha
}

#[tokio::test]
async fn list_items_carry_rating_runtime_genres_and_genre_counts() {
    let e = env(None).await;
    seed_small(&e).await;
    let (s, v) = get(&e.admin, "/api/v1/libraries/1/items?sort=title").await;
    assert_eq!(s, 200, "{v}");
    let a = &v["items"][0];
    assert_eq!(a["title"], "Alpha Thriller");
    assert_eq!(
        (a["rating"].as_f64(), a["runtime_s"].as_i64()),
        (Some(8.5), Some(7200))
    );
    assert_eq!(a["genres"], json!(["Crime", "Thriller"]));
    assert!(
        v["items"][3].get("rating").is_none(),
        "unrated items omit rating"
    );
    // other list endpoints too
    let (_, h) = get(&e.admin, "/api/v1/home").await;
    assert!(h["recently_added"][0]["genres"].is_array());
    // detail JSON must not get duplicate keys
    let raw = e
        .admin
        .clone()
        .oneshot(Request::get("/api/v1/items/1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body =
        String::from_utf8(raw.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert_eq!(body.matches("\"rating\"").count(), 1, "{body}");

    let (s, g) = get(&e.admin, "/api/v1/genres").await;
    assert_eq!(s, 200);
    assert_eq!(
        g,
        json!([
            {"name":"Crime","count":2},{"name":"Drama","count":2},{"name":"Thriller","count":2},{"name":"Comedy","count":1}
        ])
    );
    let (_, g) = get(&e.admin, "/api/v1/genres?library_id=1").await;
    assert_eq!(g.as_array().unwrap().len(), 4);
    assert_eq!(get(&e.admin, "/api/v1/genres?library_id=99").await.0, 404);
}

#[tokio::test]
async fn filters_and_sorts() {
    let e = env(None).await;
    seed_small(&e).await;
    let q = |qs: &str| {
        let app = e.admin.clone();
        let uri = format!("/api/v1/libraries/1/items?{qs}");
        async move {
            let (s, v) = get(&app, &uri).await;
            assert_eq!(s, 200, "{uri}: {v}");
            ids(&v["items"])
        }
    };
    assert_eq!(q("genre=Thriller&sort=rating").await, [1, 3]);
    assert_eq!(
        q("genre=thriller,crime").await,
        [1],
        "AND semantics, case-insensitive"
    );
    assert_eq!(
        q("genre=Thriller&genre=Crime").await,
        [1],
        "repeated parameter"
    );
    assert_eq!(q("genre=Crime&genre=Drama").await, [4]);
    assert_eq!(q("year_from=2005&year_to=2020&sort=year").await, [4, 3, 2]);
    assert_eq!(q("min_rating=7&sort=rating").await, [5, 1, 3]);
    assert_eq!(q("max_runtime_min=100&sort=runtime").await, [3, 2, 5]);
    assert_eq!(
        q("unwatched=true&sort=rating").await,
        [5, 3, 2, 4],
        "Alpha is watched; unrated last"
    );
    assert_eq!(q("sort=runtime").await, [3, 2, 5, 1, 4]);
    assert_eq!(q("sort=rating").await, [5, 1, 3, 2, 4]);
    assert_eq!(q("genre=Nope").await, Vec::<i64>::new());
    let (s, _) = get(&e.admin, "/api/v1/libraries/1/items?min_rating=abc").await;
    assert_eq!(s, 400);
    let (s, _) = get(&e.admin, "/api/v1/libraries/1/items?sort=bogus").await;
    assert_eq!(s, 400);
}

async fn collect_pages(app: &Router, base: &str, limit: usize) -> Vec<i64> {
    let mut out = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..200 {
        let mut uri = format!("{base}&limit={limit}");
        if let Some(c) = &cursor {
            uri.push_str(&format!("&cursor={c}"));
        }
        let (s, v) = get(app, &uri).await;
        assert_eq!(s, 200, "{uri}: {v}");
        out.extend(ids(&v["items"]));
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => return out,
        }
    }
    panic!("cursor loop");
}

#[tokio::test]
async fn cursor_pagination_works_with_filters_and_every_sort() {
    let e = env(None).await;
    for i in 1..=37i64 {
        let genres: &[&str] = if i % 2 == 0 {
            &["Action", "Drama"]
        } else {
            &["Drama"]
        };
        movie(
            &e.db,
            i,
            &format!("Movie {i:02}"),
            1980 + i % 30,
            Some((i % 10) as f64 + 0.5),
            80 + i,
            genres,
            None,
        )
        .await;
    }
    for (sort, desc) in [
        ("title", false),
        ("added", true),
        ("year", true),
        ("rating", true),
        ("runtime", false),
        ("random", false),
    ] {
        let base =
            format!("/api/v1/libraries/1/items?genre=Action,Drama&min_rating=2&sort={sort}&seed=7");
        let (_, whole) = get(&e.admin, &format!("{base}&limit=200")).await;
        let expect = ids(&whole["items"]);
        assert!(expect.len() > 10, "{sort}: {expect:?}");
        let paged = collect_pages(&e.admin, &base, 5).await;
        assert_eq!(
            paged, expect,
            "{sort}: pages must concatenate to the full filtered list"
        );
        let _ = desc;
    }
    // unfiltered random covers everything exactly once
    let all = collect_pages(&e.admin, "/api/v1/libraries/1/items?sort=random&seed=3", 10).await;
    assert_eq!(all.len(), 37);
    assert_eq!(all.iter().collect::<HashSet<_>>().len(), 37);
    // stable for a seed, different between seeds, not just id order
    let again = collect_pages(&e.admin, "/api/v1/libraries/1/items?sort=random&seed=3", 4).await;
    assert_eq!(all, again);
    let other = collect_pages(&e.admin, "/api/v1/libraries/1/items?sort=random&seed=4", 10).await;
    assert_ne!(all, other);
    let mut sorted = all.clone();
    sorted.sort();
    assert_ne!(all, sorted);
}

async fn tmdb_list(s: &MockServer, p: &str, ids: &[i64]) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page": 1, "results": ids.iter().map(|i| json!({"id": i, "title": format!("T{i}")})).collect::<Vec<_>>()
        })))
        .expect(1)
        .mount(s)
        .await;
}

#[tokio::test]
async fn lanes_compose_per_user_with_tmdb_recommendations() {
    let tmdb = MockServer::start().await;
    // the user last watched movie 1 (tmdb 101): recommendations 102,103,104 + similar 105,101,999
    tmdb_list(&tmdb, "/movie/101/recommendations", &[102, 103, 104]).await;
    tmdb_list(&tmdb, "/movie/101/similar", &[105, 101, 999]).await;
    let e = env(Some(&tmdb)).await;
    seed_small(&e).await;
    // enough drama/thriller/… to form genre lanes
    for i in 10..30i64 {
        movie(
            &e.db,
            i,
            &format!("Filler {i}"),
            2000 + i,
            Some(5.0 + (i % 5) as f64),
            95 + i,
            &["Drama"],
            None,
        )
        .await;
    }
    // old favourites to unlock "rediscover"
    for i in 40..47i64 {
        movie(
            &e.db,
            i,
            &format!("Classic {i}"),
            1970,
            Some(8.0),
            130,
            &["Classic"],
            None,
        )
        .await;
        watched(&e.db, 1, i, 1_500_000_000).await;
    }
    let (s, v) = get(&e.admin, "/api/v1/browse/lanes?library_id=1").await;
    assert_eq!(s, 200, "{v}");
    let lanes = v.as_array().unwrap();
    let kinds: Vec<&str> = lanes.iter().map(|l| l["kind"].as_str().unwrap()).collect();
    assert_eq!(
        &kinds[..5],
        [
            "because_you_watched",
            "top_unwatched",
            "short",
            "new",
            "rediscover"
        ],
        "{kinds:?}"
    );
    assert!(kinds[5..].iter().all(|k| *k == "genre"));
    let l = &lanes[0];
    assert_eq!(l["title"], "Because you watched Alpha Thriller");
    assert_eq!(
        ids(&l["items"]),
        [2, 3, 4, 5],
        "recommendations first, then similar, library-only, not the seed"
    );
    let top = &lanes[1];
    let r: Vec<f64> = top["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["rating"].as_f64().unwrap())
        .collect();
    assert!(
        r.windows(2).all(|w| w[0] >= w[1])
            && top["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i["played"] == false)
    );
    assert_eq!(
        top["see_all"],
        json!({"library_id": 1, "query": "sort=rating&unwatched=true"})
    );
    let short = &lanes[2];
    assert!(
        short["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["runtime_s"].as_i64().unwrap() <= 6000)
    );
    assert_eq!(
        short["see_all"]["query"],
        "max_runtime_min=100&sort=runtime"
    );
    assert_eq!(ids(&lanes[3]["items"])[0], 46, "newest first");
    assert_eq!(
        lanes[4]["items"].as_array().unwrap().len(),
        8,
        "7 classics + Alpha"
    );
    let genre = lanes.iter().find(|l| l["title"] == "Drama").unwrap();
    assert!(genre["items"].as_array().unwrap().len() <= 20);
    assert_eq!(
        genre["see_all"],
        json!({"library_id": 1, "query": "genre=Drama&sort=rating"})
    );
    assert!(lanes.len() <= 14);
    assert!(
        lanes
            .iter()
            .all(|l| l["items"].as_array().unwrap().len() <= 20)
    );
    // "see all" queries are valid item-list queries
    let q = genre["see_all"]["query"].as_str().unwrap();
    assert_eq!(
        get(&e.admin, &format!("/api/v1/libraries/1/items?{q}"))
            .await
            .0,
        200
    );

    // per-user: a fresh user has nothing watched → no because/rediscover lanes
    let u = e
        .state
        .auth
        .create_user(NewUser {
            username: "zed".into(),
            password: "another long password".into(),
            role: Role::User,
            limits: Limits::default(),
            library_ids: vec![1],
        })
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login("zed", "another long password", Default::default())
        .await
        .unwrap()
        .token;
    let zed = common::with_token(app(e.state.clone()), &t);
    let (_, v) = get(&zed, "/api/v1/browse/lanes?library_id=1").await;
    let kinds: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["kind"].as_str().unwrap())
        .collect();
    assert!(
        !kinds.contains(&"because_you_watched") && !kinds.contains(&"rediscover"),
        "{kinds:?}"
    );
    assert!(kinds.contains(&"top_unwatched"));
    let _ = u;
    // library access is enforced
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (2,'Other','movies')")
        .execute(&e.db)
        .await
        .unwrap();
    assert_eq!(get(&zed, "/api/v1/browse/lanes?library_id=2").await.0, 404);
    assert_eq!(get(&zed, "/api/v1/pick?library_id=2").await.0, 404);
    assert_eq!(get(&zed, "/api/v1/genres?library_id=2").await.0, 404);
}

#[tokio::test]
async fn lanes_without_tmdb_skip_because_you_watched_and_pick_works() {
    let e = env(None).await;
    seed_small(&e).await;
    let (_, v) = get(&e.admin, "/api/v1/browse/lanes?library_id=1").await;
    let kinds: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["top_unwatched", "short", "new"]);
    // default library when none is given
    assert_eq!(get(&e.admin, "/api/v1/browse/lanes").await.1, v);

    for _ in 0..10 {
        let (s, p) = get(
            &e.admin,
            "/api/v1/pick?library_id=1&genre=Thriller&unwatched=true&max_runtime_min=90",
        )
        .await;
        assert_eq!(s, 200, "{p}");
        assert_eq!(p["id"], 3);
        assert_eq!(p["genres"], json!(["Thriller"]));
    }
    let seen: HashSet<i64> = {
        let mut set = HashSet::new();
        for _ in 0..40 {
            set.insert(
                get(&e.admin, "/api/v1/pick?min_rating=6").await.1["id"]
                    .as_i64()
                    .unwrap(),
            );
        }
        set
    };
    assert!(seen.len() >= 3, "random picks vary: {seen:?}");
    assert_eq!(get(&e.admin, "/api/v1/pick?genre=Western").await.0, 404);
}

#[tokio::test]
async fn lanes_are_fast_on_500_items() {
    let e = env(None).await;
    let genres = [
        "Drama", "Comedy", "Action", "Thriller", "Sci-Fi", "Horror", "Romance", "Crime",
    ];
    let mut tx = e.db.begin().await.unwrap();
    for i in 1..=500i64 {
        sqlx::query("INSERT INTO media_items (id, library_id, kind, title, sort_title, year, rating, runtime_s, poster, added_at) VALUES (?,1,'movie',?,lower(?),?,?,?,'/p',?)")
            .bind(i).bind(format!("Movie {i}")).bind(format!("Movie {i}")).bind(1980 + i % 40).bind(3.0 + (i % 70) as f64 / 10.0).bind((70 + i % 90) * 60).bind(i)
            .execute(&mut *tx).await.unwrap();
        for k in 0..2 {
            let g = genres[((i + k * 3) % 8) as usize];
            sqlx::query("INSERT OR IGNORE INTO genres (name) VALUES (?)")
                .bind(g)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("INSERT OR IGNORE INTO item_genres (item_id, genre_id) SELECT ?, id FROM genres WHERE name = ?").bind(i).bind(g).execute(&mut *tx).await.unwrap();
        }
        if i % 4 == 0 {
            sqlx::query("INSERT INTO watch_state (user_id, item_id, position_s, played, last_played_at) VALUES (1,?,0,1,1500000000)").bind(i).execute(&mut *tx).await.unwrap();
        }
    }
    tx.commit().await.unwrap();
    let _ = get(&e.admin, "/api/v1/browse/lanes?library_id=1").await; // warm up
    let t = std::time::Instant::now();
    let (s, v) = get(&e.admin, "/api/v1/browse/lanes?library_id=1").await;
    let took = t.elapsed();
    assert_eq!(s, 200);
    assert!(
        v.as_array().unwrap().len() >= 8,
        "{}",
        v.as_array().unwrap().len()
    );
    assert!(took.as_millis() < 200, "lanes took {took:?}");
    let t = std::time::Instant::now();
    let (s, _) = get(
        &e.admin,
        "/api/v1/libraries/1/items?genre=Drama&unwatched=true&sort=rating&limit=50",
    )
    .await;
    assert_eq!(s, 200);
    assert!(
        t.elapsed().as_millis() < 200,
        "filtered list took {:?}",
        t.elapsed()
    );
}

async fn people_seed(e: &Env) {
    let db = &e.db;
    movie(
        db,
        1,
        "Iron Man",
        2008,
        Some(7.9),
        126,
        &["Action"],
        Some(1726),
    )
    .await;
    movie(
        db,
        2,
        "Sherlock Holmes",
        2009,
        Some(7.6),
        128,
        &["Action"],
        Some(10528),
    )
    .await;
    movie(
        db,
        3,
        "Downey Street",
        2001,
        Some(5.0),
        90,
        &["Drama"],
        Some(777),
    )
    .await;
    movie(
        db,
        4,
        "Amélie",
        2001,
        Some(8.3),
        122,
        &["Comedy"],
        Some(194),
    )
    .await;
    movie(
        db,
        5,
        "Zodiac",
        2007,
        Some(7.7),
        157,
        &["Crime"],
        Some(1949),
    )
    .await;
    sqlx::query("INSERT INTO people (id, tmdb_id, name) VALUES (1,3223,'Robert Downey Jr.'),(2,NULL,'Local Only Person'),(3,1,'Zoé Zodiac')").execute(db).await.unwrap();
    sqlx::query("INSERT INTO credits (item_id, person_id, role, ord) VALUES (1,1,'actor',0),(2,1,'actor',0),(5,1,'actor',1),(5,2,'actor',2),(4,3,'actor',0)").execute(db).await.unwrap();
    for i in 1..=5 {
        sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (?, ?, 1, 1)")
            .bind(i)
            .bind(format!("/f{i}.mkv"))
            .execute(db)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn search_all_ranks_titles_then_cast_with_diacritics_and_episodes() {
    let e = env(None).await;
    people_seed(&e).await;
    sqlx::query(
        "INSERT INTO media_items (id, library_id, kind, title) VALUES (60,1,'show','Zeta Show')",
    )
    .execute(&e.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no) VALUES (61,1,'season',60,'Season 1',1)").execute(&e.db).await.unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no, episode_no) VALUES (62,1,'episode',61,'The Iron Giant Returns',1,1)").execute(&e.db).await.unwrap();
    let s = |q: &str| {
        let app = e.admin.clone();
        let uri = format!("/api/v1/search/all?q={q}");
        async move {
            let (c, v) = get(&app, &uri).await;
            assert_eq!(c, 200, "{v}");
            v
        }
    };
    // title beats cast: "downey" → the film titled Downey Street first, then Downey's own films
    let v = s("downey").await;
    assert_eq!(ids(&v["library"]), [3, 1, 5, 2], "{v}");
    // diacritics + case
    assert_eq!(ids(&s("AMELIE").await["library"]), [4]);
    assert_eq!(ids(&s("am%C3%A9lie").await["library"]), [4]);
    // ranking: exact > prefix > word-prefix > substring
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, rating) VALUES (70,1,'movie','Man',9),(71,1,'movie','Manhattan',9),(72,1,'movie','Iron Man 2',9),(73,1,'movie','Superman',9)").execute(&e.db).await.unwrap();
    assert_eq!(
        ids(&s("man").await["library"]),
        [70, 71, 72, 1, 73],
        "exact, prefix, word-prefix (by rating), substring: {:?}",
        s("man").await
    );
    // episodes
    let v = s("giant").await;
    assert_eq!(ids(&v["episodes"]), [62]);
    assert!(ids(&v["library"]).is_empty());
    // people: local people with a TMDB id and library credits (no TMDB configured → still works)
    let v = s("downey").await;
    let p = &v["people"][0];
    assert_eq!(
        (
            p["tmdb_id"].as_i64(),
            p["name"].as_str(),
            p["in_library_count"].as_i64()
        ),
        (Some(3223), Some("Robert Downey Jr."), Some(3))
    );
    assert_eq!(
        p["known_for"],
        json!(["Iron Man", "Zodiac", "Sherlock Holmes"])
    );
    assert_eq!(v["discover"], json!([]));
    assert!(
        s("local%20only").await["people"]
            .as_array()
            .unwrap()
            .is_empty(),
        "no tmdb id → no person page"
    );
    assert_eq!(
        s("%20").await,
        json!({"library": [], "episodes": [], "people": [], "discover": [], "vod": []})
    );
    // the old endpoint still works
    let (c, old) = get(&e.admin, "/api/v1/search?q=zodiac").await;
    assert_eq!(c, 200);
    assert_eq!(ids(&old), [5]);
}

#[tokio::test]
async fn search_all_merges_tmdb_people_and_discover_and_survives_tmdb_failure() {
    let tmdb = MockServer::start().await;
    Mock::given(method("GET")).and(path("/search/person")).and(query_param("query", "downey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 9999, "name": "Some Other Downey", "profile_path": "/o.jpg", "known_for_department": "Acting", "known_for": [{"id": 1, "title": "Other Film"}]},
            {"id": 3223, "name": "Robert Downey Jr.", "profile_path": "/rdj.jpg", "known_for_department": "Acting",
             "known_for": [{"id": 1726, "title": "Iron Man"}, {"id": 24428, "title": "The Avengers"}, {"id": 5, "name": "Show"}, {"id": 6, "title": "Fourth"}]}
        ]})))
        .expect(1) // second identical search is served from the 10 min cache
        .mount(&tmdb).await;
    Mock::given(method("GET")).and(path("/search/movie")).and(query_param("query", "downey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 1726, "title": "Iron Man", "release_date": "2008-05-02", "poster_path": "/im.jpg", "popularity": 90.0},
            {"id": 555, "title": "Not In Library", "release_date": "2010-01-01", "poster_path": "/n.jpg", "popularity": 50.0}
        ]})))
        .expect(1).mount(&tmdb).await;
    Mock::given(method("GET")).and(path("/search/tv")).and(query_param("query", "downey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 600, "name": "A Series", "first_air_date": "2015-01-01", "poster_path": "/s.jpg", "popularity": 70.0}
        ]})))
        .expect(1).mount(&tmdb).await;
    let e = env(Some(&tmdb)).await;
    people_seed(&e).await;
    for _ in 0..2 {
        let (c, v) = get(&e.admin, "/api/v1/search/all?q=downey").await;
        assert_eq!(c, 200, "{v}");
        let people = v["people"].as_array().unwrap();
        assert_eq!(people[0]["tmdb_id"], 3223, "in-library people first");
        assert_eq!(people[0]["in_library_count"], 3);
        assert_eq!(
            people[0]["known_for"],
            json!(["Iron Man", "The Avengers", "Show"])
        );
        assert!(
            people[0]["photo_url"]
                .as_str()
                .unwrap()
                .contains("/images/tmdb/profile")
        );
        assert_eq!(people[0]["known_for_department"], "Acting");
        assert_eq!(
            (
                people[1]["tmdb_id"].as_i64(),
                people[1]["in_library_count"].as_i64()
            ),
            (Some(9999), Some(0))
        );
        assert_eq!(people.len(), 2, "merged by tmdb id, no duplicate Downey");
        let d: Vec<i64> = v["discover"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["tmdb_id"].as_i64().unwrap())
            .collect();
        assert_eq!(
            d,
            [600, 555],
            "Iron Man is in the library → not offered again; popularity order: {v}"
        );
        assert_eq!(v["discover"][0]["status"], "none");
        assert!(
            v["discover"][0]["poster_url"]
                .as_str()
                .unwrap()
                .contains("/images/tmdb/poster")
        );
    }

    // TMDB down: library results still come back
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&bad)
        .await;
    let e2 = env(Some(&bad)).await;
    people_seed(&e2).await;
    let (c, v) = get(&e2.admin, "/api/v1/search/all?q=downey").await;
    assert_eq!(c, 200);
    assert_eq!(ids(&v["library"]), [3, 1, 5, 2]);
    assert_eq!(v["discover"], json!([]));
    assert_eq!(v["people"][0]["tmdb_id"], 3223, "local people survive");
}

#[tokio::test]
async fn search_all_respects_library_access_and_auth() {
    let e = env(None).await;
    people_seed(&e).await;
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (2,'Hidden','movies')")
        .execute(&e.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title) VALUES (90,2,'movie','Secret Iron Film')").execute(&e.db).await.unwrap();
    e.state
        .auth
        .create_user(NewUser {
            username: "kim".into(),
            password: "another long password".into(),
            role: Role::User,
            limits: Limits::default(),
            library_ids: vec![1],
        })
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login("kim", "another long password", Default::default())
        .await
        .unwrap()
        .token;
    let kim = common::with_token(app(e.state.clone()), &t);
    let (_, v) = get(&kim, "/api/v1/search/all?q=iron").await;
    assert_eq!(ids(&v["library"]), [1]);
    let (_, v) = get(&e.admin, "/api/v1/search/all?q=iron").await;
    assert_eq!(
        ids(&v["library"]),
        [1, 90],
        "prefix 'Iron Man' before word-prefix 'Secret Iron Film'"
    );
    assert!(ids(&v["library"]).contains(&90));
    let raw = app(e.state.clone());
    for p in [
        "/api/v1/search/all?q=x",
        "/api/v1/genres",
        "/api/v1/browse/lanes",
        "/api/v1/pick",
    ] {
        assert_eq!(get(&raw, p).await.0, 401, "{p}");
    }
}
