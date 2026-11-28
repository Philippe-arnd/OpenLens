//! The event budget (#702, #703, D-107): above about 2,000 active users a
//! day the SDK sends `$vital`, `$scroll` and `$field` from a sample of
//! sessions and strips the click position outside it, and it counts the
//! repeats of an error instead of sending them. The numbers that count
//! people must not move, and error counts must still add up.

use super::*;
use openlens_engine::error_groups;
use openlens_engine::forms::forms;

/// Who did what, the same in every variant: four people over five weeks,
/// some sessions bouncing, one with an error, one filling a form.
fn sessions() -> Vec<(&'static str, &'static str, i64)> {
    let mut out = Vec::new();
    for (user, weeks) in [("ann", 4), ("bob", 2), ("cy", 0), ("anon-1", 1)] {
        for w in 0..=weeks {
            out.push((user, w, w * 7 * 24 * 60 + 30));
        }
    }
    out.into_iter()
        .map(|(u, w, m)| {
            let s: &'static str = Box::leak(format!("{u}-{w}").into_boxed_str());
            (u, s, m)
        })
        .collect()
}

/// The events of one session: a page view, a click, a form field left and
/// sent, scroll and vitals, and for `ann` an error. `drawn` says whether the
/// session sends the distribution-only events; `sample` is the rate they
/// carry when it is below 1.
fn session(
    user: &'static str,
    sid: &'static str,
    mins: i64,
    drawn: bool,
    sample: Option<f64>,
) -> Vec<(E<'static>, String)> {
    let e = |event: &'static str, m: i64, props: String| {
        (
            E {
                user,
                role: "member",
                session: sid,
                event,
                mins_ago: m,
                feature: None,
                release: "v1",
                props: "",
            },
            props,
        )
    };
    let tag = sample.map_or_else(String::new, |r| format!(r#","$sample":{r}"#));
    let mut out = Vec::new();
    if !user.starts_with("anon") {
        out.push(e("$identify", mins, "{}".into()));
    }
    out.push(e("$pageview", mins, "{}".into()));
    // Bounces: bob's sessions have one interaction only.
    if user != "bob" {
        let pos = if drawn {
            r#","$x":0.5,"$y":120,"$vw":1280"#
        } else {
            ""
        };
        out.push(e(
            "$click",
            mins - 1,
            format!(r#"{{"$tag":"button"{pos}}}"#),
        ));
    }
    if user == "ann" {
        out.push(e(
            "$error",
            mins - 2,
            r#"{"$message":"boom","$source":"/a.js:1:2"}"#.into(),
        ));
    }
    if drawn {
        out.push(e(
            "$field",
            mins - 2,
            format!(r#"{{"$form":"signup","$field":"email","$ms":900{tag}}}"#),
        ));
        out.push(e("$scroll", mins - 3, format!(r#"{{"$depth":80{tag}}}"#)));
        out.push(e(
            "$vital",
            mins - 3,
            format!(r#"{{"$name":"LCP","$value":1800,"$rating":"good"{tag}}}"#),
        ));
    }
    if user == "ann" {
        out.push(e("$submit", mins - 3, r#"{"$form":"signup"}"#.into()));
    }
    out
}

/// Store the sessions, each drawn when `drawn(i)` says so.
fn store(drawn: impl Fn(usize) -> bool, sample: Option<f64>) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for (i, (u, s, m)) in sessions().into_iter().enumerate() {
        for (j, (mut e, props)) in session(u, s, m, drawn(i), sample).into_iter().enumerate() {
            let props: &'static str = Box::leak(props.into_boxed_str());
            e.props = props;
            let mut ev = mk(&e, j);
            ev.uuid = format!("{s}-{j}");
            evs.push(ev);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

/// The people-counting numbers of a board, as text to compare.
async fn people(q: &QueryEngine) -> String {
    let b = board(q, "p", &Filters::default(), None).await.unwrap();
    let u = &b.users;
    format!(
        "users {} {} {} {} {} {} | sessions {} bounce {:.6} error {:.6} | dau {:?} | retention {:?}",
        u.dau,
        u.wau,
        u.mau,
        u.anonymous,
        u.known,
        u.resurrected,
        b.sessions.count,
        b.sessions.bounce_rate,
        b.sessions.error_rate,
        b.dau_series,
        b.retention
            .iter()
            .map(|r| (r.week.clone(), r.users, r.weeks.clone()))
            .collect::<Vec<_>>()
    )
}

#[tokio::test]
async fn sampling_distribution_events_moves_no_number_that_counts_people() {
    let (_a, all) = store(|_| true, None);
    // One session in four drawn, as with 8,000 active users a day.
    let (_b, sampled) = store(|i| i.is_multiple_of(4), Some(0.25));
    // And none drawn at all.
    let (_c, none) = store(|_| false, Some(0.25));
    let reference = people(&all).await;
    assert!(
        !reference.starts_with("users 0 "),
        "the dataset has users: {reference}"
    );
    assert_eq!(people(&sampled).await, reference);
    assert_eq!(people(&none).await, reference);
}

#[tokio::test]
async fn sampled_form_fields_are_scaled_back_up() {
    let (_a, all) = store(|_| true, None);
    let (_b, sampled) = store(|i| i.is_multiple_of(4), Some(0.25));
    let f = forms(&all, "p", None, &Filters::default()).await.unwrap();
    let g = forms(&sampled, "p", None, &Filters::default())
        .await
        .unwrap();
    let (f, g) = (&f[0], &g[0]);
    // Fewer sessions measured, each standing for four: about as many starts.
    assert!(
        g.starts >= f.starts / 2 && g.starts <= f.starts * 2,
        "{f:?} {g:?}"
    );
    assert!(
        g.starts > 1,
        "a measured session stands for more than one: {g:?}"
    );
}

/// One error row per session and fingerprint, plus one row with the
/// repeats (#703): the group counts every occurrence, the error rate and
/// the users hit stay those of the first rows.
#[tokio::test]
async fn repeated_errors_count_their_repeats_once() {
    let write = |repeats: bool| {
        let dir = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
        let e = |user, session, event, mins_ago, props| E {
            user,
            role: "member",
            session,
            event,
            mins_ago,
            feature: None,
            release: "v1",
            props,
        };
        let mut rows = vec![
            e("ann", "s1", "$identify", 30, "{}"),
            e("ann", "s1", "$pageview", 30, "{}"),
            e(
                "ann",
                "s1",
                "$error",
                29,
                r#"{"$message":"boom","$source":"/a.js:1:2"}"#,
            ),
            e("bob", "s2", "$identify", 20, "{}"),
            e("bob", "s2", "$pageview", 20, "{}"),
        ];
        if repeats {
            rows.push(e(
                "ann",
                "s1",
                "$error",
                28,
                r#"{"$message":"boom","$source":"/a.js:1:2","$repeat":true,"$repeats":41}"#,
            ));
        }
        let evs: Vec<_> = rows.iter().enumerate().map(|(i, e)| mk(e, i)).collect();
        w.append(&evs).unwrap();
        w.flush().unwrap();
        let q = QueryEngine::new(dir.path(), Limits::default());
        (dir, q)
    };
    let (_a, once) = write(false);
    let (_b, repeated) = write(true);
    let f = Filters::default();
    let g1 = error_groups(&once, "p", "events", &f).await.unwrap();
    let g2 = error_groups(&repeated, "p", "events", &f).await.unwrap();
    assert_eq!(g2.len(), g1.len(), "the repeat row is in the same group");
    let (g1, g2) = (&g1[0], &g2[0]);
    assert_eq!(g2.count, g1.count + 41, "every occurrence counts");
    assert_eq!((g2.users, g2.sessions), (g1.users, g1.sessions));
    let (b1, b2) = (
        board(&once, "p", &f, None).await.unwrap(),
        board(&repeated, "p", &f, None).await.unwrap(),
    );
    assert_eq!(b2.sessions.error_rate, b1.sessions.error_rate);
    assert_eq!(b2.sessions.bounce_rate, b1.sessions.bounce_rate);
}
