//! What the daily per-person rollup (#451, D-038) takes from the feature
//! map: the same feature lookup and task definition as the board, written
//! out once per pass.

use openlens_featuremap::FeatureMap;
use openlens_query::DayFeatures;

/// How `person_day` resolves features and counts tasks with `map` (and its
/// version), or without a map.
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
