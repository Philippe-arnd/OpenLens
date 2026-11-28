//! The portfolio across projects (#717, D-118): a person sees only the
//! projects they may see, and the page never computes a board.

use super::*;

async fn json(r: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap()).unwrap()
}

#[tokio::test]
async fn a_viewer_of_one_project_of_three_sees_only_that_one() {
    let h = harness();
    h.state.meta.create_project("Blog", None).unwrap();
    h.state.meta.create_project("Docs", None).unwrap();
    let viewer = invite(
        &h,
        serde_json::json!({"email": "viewer@example.com", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    let owner = login_cookie(&h).await;

    let mine = get(&h, "/portfolio", Some(&viewer), false).await;
    assert_eq!(mine.status(), StatusCode::OK);
    let mine = body_text(mine).await;
    assert!(mine.contains("href=\"/p/default\""), "{mine}");
    assert!(!mine.contains("/p/blog") && !mine.contains("/p/docs"));
    // One project: no portfolio link in the sidebar.
    assert!(!mine.contains("href=\"/portfolio\""));

    let all = body_text(get(&h, "/portfolio", Some(&owner), false).await).await;
    for id in ["default", "blog", "docs"] {
        assert!(all.contains(&format!("href=\"/p/{id}\"")), "{id}");
    }
    assert!(all.contains("href=\"/portfolio\""), "the sidebar links it");

    // The API: the admin token sees every project.
    let v = json(get(&h, "/api/portfolio", None, true).await).await;
    assert_eq!(v["projects"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        get(&h, "/api/portfolio", None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_project_without_a_kept_board_shows_computing_and_nothing_is_computed() {
    let h = harness();
    h.state.meta.create_project("Blog", None).unwrap();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let boards = h.state.boards.clone();
    let before = boards.computed();

    let page = body_text(get(&h, "/portfolio", Some(&owner), false).await).await;
    assert!(page.contains("Computing"), "{page}");
    let v = json(get(&h, "/api/portfolio", None, true).await).await;
    let rows = v["projects"].as_array().unwrap();
    assert!(rows.iter().all(|r| r["computing"] == true), "{v}");
    // Nothing started behind the page either.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(boards.computed(), before, "the portfolio computed a board");
    assert!(boards.is_empty());

    // Once the board page computed the default board, the portfolio reads
    // it from the cache, and still computes nothing.
    // A cold board page streams: the board is computed as its body is read.
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(!board.contains("t show that"), "the board was computed");
    let after_board = boards.computed();
    assert!(after_board > before);
    let v = json(get(&h, "/api/portfolio", None, true).await).await;
    let row = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"] == "default")
        .cloned()
        .unwrap();
    assert_eq!(row["computing"], false, "{row}");
    assert!(row["outcomes"]["reach"].as_i64().is_some(), "{row}");
    let blog = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"] == "blog")
        .cloned()
        .unwrap();
    assert_eq!(blog["computing"], true);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(boards.computed(), after_board);
}

#[tokio::test]
async fn the_digest_without_a_project_has_a_section_per_project() {
    let h = harness();
    h.state.meta.create_project("Blog", None).unwrap();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let v = json(get(&h, "/api/digest", None, true).await).await;
    let sections = v["projects"].as_array().unwrap();
    assert_eq!(sections.len(), 2, "{v}");
    let md = v["markdown"].as_str().unwrap();
    assert!(md.contains("## default: your week") && md.contains("## Blog: your week"));
    // One project's digest stays as it was.
    let one = json(get(&h, "/api/projects/default/digest", None, true).await).await;
    assert!(
        one["markdown"]
            .as_str()
            .unwrap()
            .starts_with("# default: your week")
    );
}
