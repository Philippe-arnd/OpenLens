//! Significance tests for comparing two cohorts (experiments = flags).
//!
//! Large-sample tests on sessions: a two-proportion z-test for rates (share
//! of sessions with an error) and Welch's test with the normal approximation
//! for means (tasks per session). Both are two-sided. With fewer than 30
//! sessions on a side the result is `None`: too small to say.

/// Fewest sessions per side before a test is run.
pub const MIN_SESSIONS: i64 = 30;
/// Threshold under which a difference is called significant.
pub const ALPHA: f64 = 0.05;

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
    two_proportions_min(p1, n1, p2, n2, MIN_SESSIONS)
}

/// [`two_proportions`] with `min` units per side instead of
/// [`MIN_SESSIONS`]: the insight cards compare groups of users, with
/// their own minimum (`insights::MIN_GROUP`, #713).
pub fn two_proportions_min(p1: f64, n1: i64, p2: f64, n2: i64, min: i64) -> Option<f64> {
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
}
