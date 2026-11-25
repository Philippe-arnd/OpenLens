use super::*;

#[tokio::test]
async fn one_cold_query_at_a_time_and_hot_ones_never_wait() {
    let dir = tempfile::tempdir().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let held = q.cold_turn.try_acquire().unwrap();
    assert!(matches!(
        q.cold().sql("a", "SELECT 1 AS n").await,
        Err(QueryError::Busy)
    ));
    assert_eq!(q.sql("a", "SELECT 1 AS n").await.unwrap().rows[0]["n"], 1);
    drop(held);
    assert_eq!(
        q.cold().sql("a", "SELECT 1 AS n").await.unwrap().rows[0]["n"],
        1
    );
}

/// #458: a cold query on one engine does not make another engine's
/// cold query busy (the server's tests run many engines at once).
#[tokio::test]
async fn engines_take_cold_turns_on_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let a = QueryEngine::new(dir.path(), Limits::default());
    let b = QueryEngine::new(dir.path(), Limits::default());
    let _held = a.cold_turn.try_acquire().unwrap();
    assert!(matches!(
        a.cold().sql("p", "SELECT 1 AS n").await,
        Err(QueryError::Busy)
    ));
    assert_eq!(
        b.cold().sql("p", "SELECT 1 AS n").await.unwrap().rows[0]["n"],
        1
    );
}

#[test]
fn running_out_of_memory_is_said_plainly() {
    let e = friendly(QueryError::Rejected(
        "Resources exhausted: Failed to allocate 1 GB for HashJoin".into(),
    ));
    assert!(matches!(e, QueryError::TooBig));
    assert!(matches!(
        friendly(QueryError::Rejected("syntax".into())),
        QueryError::Rejected(_)
    ));
}
