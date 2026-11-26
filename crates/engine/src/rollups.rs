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

#[cfg(test)]
mod tests {
    /// `session_day` counts interactions with the query crate's rule; the
    /// board's sessions must use the same one, or a session rolled up and
    /// the same session read from the events disagree on bounces.
    #[test]
    fn the_board_and_session_day_agree_on_interactions() {
        assert_eq!(crate::INTERACTION, openlens_query::INTERACTION);
    }
}
