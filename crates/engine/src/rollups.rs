//! What the daily rollups per person (#451, D-038) and per session (#493,
//! D-041) take from the feature map: the same feature lookup and task
//! definition as the board, written out once per pass.

use openlens_featuremap::FeatureMap;
use openlens_query::DayFeatures;

/// How `person_day` resolves features, and how it and `session_day` count
/// tasks, with `map` (and its version), or without a map.
pub fn day_features(map: Option<(&FeatureMap, i64)>) -> DayFeatures {
    let Some((m, version)) = map else {
        return DayFeatures::default();
    };
    DayFeatures {
        feature: crate::lookups(m).1,
        successes: m
            .features
            .iter()
            .filter_map(|f| f.success.clone())
            .collect(),
        map_version: Some(version),
    }
}
