//! Significance tests for comparing two cohorts (experiments = flags).
//!
//! Large-sample tests on sessions: a two-proportion z-test for rates (share
//! of sessions with an error) and Welch's test with the normal approximation
//! for means (tasks per session). Both are two-sided. With fewer than 30
//! sessions on a side the result is `None`: too small to say.
//!
//! A card family that tests many candidates at once (every feature, every
//! role, every flag) holds its p-values to a Benjamini–Hochberg cutoff at
//! a [`FDR`] false discovery rate rather than to [`ALPHA`] each
//! ([`fdr_cutoff`], D-109), so a big app does not get cards by chance.

/// Fewest sessions per side before a test is run.
pub const MIN_SESSIONS: i64 = 30;
/// Threshold under which a difference is called significant.
pub const ALPHA: f64 = 0.05;
/// False discovery rate a card family is held to (Benjamini–Hochberg, D-109).
pub const FDR: f64 = 0.05;

/// Complementary error function (Abramowitz & Stegun 7.1.26, |error| < 1.5e-7).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let r = poly * (-z * z).exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// Two-sided p-value of a standard normal statistic.
pub fn p_from_z(z: f64) -> f64 {
    erfc(z.abs() / std::f64::consts::SQRT_2).clamp(0.0, 1.0)
}

/// Two-proportion z-test: rates `p1` over `n1` sessions vs `p2` over `n2`.
pub fn two_proportions(p1: f64, n1: i64, p2: f64, n2: i64) -> Option<f64> {
    two_proportions_from(MIN_SESSIONS, p1, n1, p2, n2)
}

/// The same test with `min` per side instead of [`MIN_SESSIONS`]: the
/// board's user cohorts, which already need
/// [`crate::insights::MIN_GROUP`] users on each side (D-109).
pub fn two_proportions_from(min: i64, p1: f64, n1: i64, p2: f64, n2: i64) -> Option<f64> {
    if n1 < min.max(1) || n2 < min.max(1) {
        return None;
    }
    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let pooled = (p1 * n1f + p2 * n2f) / (n1f + n2f);
    let se = (pooled * (1.0 - pooled) * (1.0 / n1f + 1.0 / n2f)).sqrt();
    if se <= 0.0 || !se.is_finite() {
        return None;
    }
    Some(p_from_z((p1 - p2) / se))
}

/// Welch's test for two means with standard deviations `s1`, `s2`.
pub fn two_means(m1: f64, s1: f64, n1: i64, m2: f64, s2: f64, n2: i64) -> Option<f64> {
    if n1 < MIN_SESSIONS || n2 < MIN_SESSIONS {
        return None;
    }
    let se = (s1 * s1 / n1 as f64 + s2 * s2 / n2 as f64).sqrt();
    if se <= 0.0 || !se.is_finite() {
        return None;
    }
    Some(p_from_z((m1 - m2) / se))
}

/// The Benjamini–Hochberg cutoff of a family of p-values at false
/// discovery rate `q` (D-109): the largest `p(k)` (k-th smallest) under
/// `k·q/m`. Every p-value at or under it is a discovery; `None` when there
/// is none. A p-value that is not a number counts as a test that found
/// nothing.
#[must_use]
pub fn fdr_cutoff(ps: &[f64], q: f64) -> Option<f64> {
    let mut sorted: Vec<f64> = ps
        .iter()
        .map(|p| {
            if p.is_finite() {
                p.clamp(0.0, 1.0)
            } else {
                1.0
            }
        })
        .collect();
    sorted.sort_by(f64::total_cmp);
    let m = sorted.len() as f64;
    sorted
        .iter()
        .enumerate()
        .rev()
        .find(|(i, p)| **p < (*i as f64 + 1.0) * q / m)
        .map(|(_, p)| *p)
}

/// `p` is a discovery under `cutoff` (from [`fdr_cutoff`]).
#[must_use]
pub fn discovered(cutoff: Option<f64>, p: f64) -> bool {
    cutoff.is_some_and(|c| p <= c)
}

/// Which of `ps` are discoveries at false discovery rate `q`.
#[must_use]
pub fn benjamini_hochberg(ps: &[f64], q: f64) -> Vec<bool> {
    let cut = fdr_cutoff(ps, q);
    ps.iter().map(|p| discovered(cut, *p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_tail_matches_tables() {
        assert!((p_from_z(1.96) - 0.05).abs() < 1e-3);
        assert!((p_from_z(0.0) - 1.0).abs() < 1e-6);
        assert!((p_from_z(-2.576) - 0.01).abs() < 1e-3);
    }

    #[test]
    fn clear_and_unclear_differences() {
        // 10% vs 5% errors over 1000 sessions each: clearly different.
        assert!(two_proportions(0.10, 1000, 0.05, 1000).unwrap() < 0.001);
        // 6% vs 5% over 100 each: could be chance.
        assert!(two_proportions(0.06, 100, 0.05, 100).unwrap() > 0.5);
        // Too small to say.
        assert!(two_proportions(0.5, 10, 0.1, 1000).is_none());
        // 2.0 vs 1.5 tasks, sd 1, 400 sessions each: significant.
        assert!(two_means(2.0, 1.0, 400, 1.5, 1.0, 400).unwrap() < 0.001);
        assert!(two_means(1.55, 1.0, 50, 1.5, 1.0, 50).unwrap() > 0.5);
    }

    /// Pseudo-random p-values in [0, 1), some of them small.
    fn families() -> Vec<Vec<f64>> {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % 1_000_000) as f64 / 1_000_000.0
        };
        (1..60)
            .map(|m| {
                (0..m)
                    .map(|i| if i % 3 == 0 { next() * 0.02 } else { next() })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn false_discovery_control_never_finds_more_than_the_raw_threshold() {
        for ps in families() {
            let bh = benjamini_hochberg(&ps, FDR);
            for (p, d) in ps.iter().zip(&bh) {
                assert!(!d || *p < ALPHA, "a discovery passes the raw test: {p}");
            }
            let raw = ps.iter().filter(|p| **p < ALPHA).count();
            assert!(bh.iter().filter(|d| **d).count() <= raw, "{ps:?}");
        }
        // One test alone: the plain threshold.
        assert_eq!(benjamini_hochberg(&[0.049], FDR), [true]);
        assert_eq!(benjamini_hochberg(&[0.051], FDR), [false]);
    }

    #[test]
    fn false_discovery_control_is_monotonic() {
        for ps in families() {
            let bh = benjamini_hochberg(&ps, FDR);
            // A smaller p-value than a discovery is a discovery too.
            for (i, p) in ps.iter().enumerate() {
                for (j, d) in bh.iter().enumerate() {
                    if *d && *p <= ps[j] {
                        assert!(bh[i], "{p} under discovered {}", ps[j]);
                    }
                }
            }
            // More tests that find nothing never add discoveries.
            let mut more = ps.clone();
            more.extend([1.0, 0.9, f64::NAN]);
            let found = |v: &[f64]| benjamini_hochberg(v, FDR).iter().filter(|d| **d).count();
            assert!(found(&more) <= found(&ps), "{ps:?}");
            // A looser rate never finds fewer.
            let loose = benjamini_hochberg(&ps, 0.2).iter().filter(|d| **d).count();
            assert!(loose >= found(&ps));
        }
        assert!(fdr_cutoff(&[], FDR).is_none());
    }
}
