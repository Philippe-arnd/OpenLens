//! Capacity follows the host (#606, D-088): one disk budget drives how
//! many recordings a project keeps a day. Retention does not follow it:
//! 400 days of events and 30 days of recordings, whatever the disk
//! (D-015, `docs/privacy.md`), since keeping personal data longer is no
//! benefit and an operator must be able to state a fixed duration.
//!
//! The budget is a share of the data volume (80 % by default, or
//! `OPENLENS_DISK_BUDGET`) minus what other files on the volume use. Half
//! of what is left after OpenLens's own other files (rollups, `meta.db`,
//! backups) is for events and recordings, the other half for the copy made
//! before each upgrade. Once events (400 days) and the floor of
//! recordings (300 a day, 30 days) are paid for, from the bytes measured
//! on the disk, the room left buys more recordings a day: each one more a
//! day, in every project, takes 30 days of that project's average
//! recording. Never below the floor; a filling disk brings the number back
//! down to it, and below that the low-space guard stops writes as before.
//!
//! Every project keeps the same number a day, and gets a share of the
//! budget in proportion to its cost at the floor.
//!
//! The route-stats window (#628, D-091) is chosen with the same budget,
//! before the extra recordings: see [`window`].
//!
//! The functions here are pure: the host measures, the engine derives.

use serde::{Deserialize, Serialize};

pub mod window;
pub use window::{StatsLoad, stats_load};

/// Recordings a project keeps a day, at least (D-035).
pub const REPLAYS_PER_DAY_FLOOR: i64 = crate::replay_rate::DAILY_BUDGET;
/// Recordings a project keeps a day, at most: beyond, a replay list is
/// more than anyone watches.
pub const REPLAYS_PER_DAY_CEILING: i64 = 30_000;
/// Days recordings are kept: fixed (D-015).
pub const REPLAY_RETENTION_DAYS: i64 = openlens_replay::RETENTION_DAYS;
/// Days events are kept: fixed (D-015). The same as
/// `openlens_storage::EVENT_RETENTION_DAYS`, which the retention pass uses
/// (a test holds them equal).
pub const EVENT_RETENTION_DAYS: i64 = 400;

/// Share of the volume OpenLens may use by default.
pub const DEFAULT_SHARE: f64 = 0.80;
/// Writes stop below this share of free space on the data volume, so the
/// WAL, compaction and backups still have room (D-014). The budget never
/// reaches into it.
pub const MIN_FREE_SHARE: f64 = 0.05;
/// The override's name.
pub const OVERRIDE_VAR: &str = "OPENLENS_DISK_BUDGET";
/// Shares the override may set, in percent.
const SHARE_RANGE: std::ops::RangeInclusive<f64> = 1.0..=95.0;

/// The data volume: free and total bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    /// Bytes free for OpenLens.
    pub available: u64,
    /// Size of the volume.
    pub total: u64,
}

/// What one project's data takes on the disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectUse {
    /// Project id.
    pub project: String,
    /// Bytes of its event days on the disk.
    pub event_bytes: u64,
    /// Bytes of its recordings.
    pub replay_bytes: u64,
    /// Bytes of events a day (the last closed days on the disk).
    pub event_bytes_per_day: u64,
    /// Bytes of recordings a day (the last days' recordings).
    pub replay_bytes_per_day: u64,
    /// Bytes of one recording, on average (the last days' recordings).
    pub replay_bytes_per_recording: u64,
    /// Its route stats over the last 24 hours (#628), once counted.
    #[serde(default)]
    pub stats: Option<StatsLoad>,
}

/// What the host measured on the disk (cached: at most hourly).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Bytes under the data directory.
    pub used: u64,
    /// Bytes of the copy made before the last upgrade (`backups/upgrade`),
    /// part of `used`.
    pub upgrade_copy: u64,
    /// Each project's data.
    pub projects: Vec<ProjectUse>,
    /// When it was measured (RFC 3339).
    pub measured_at: String,
}

/// What the derivation reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inputs {
    /// The data volume, when its size can be read.
    pub volume: Option<Volume>,
    /// The last measurement, once there is one.
    pub usage: Option<Usage>,
    /// `OPENLENS_DISK_BUDGET` as set (`None` when unset or empty).
    pub budget_override: Option<String>,
}

/// One value in force and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cap {
    /// The value in force.
    pub value: i64,
    /// Never less.
    pub floor: i64,
    /// Never more.
    pub ceiling: i64,
    /// Whether it follows the disk; `false` for a fixed value, or when the
    /// floor applies because something could not be known.
    pub derived: bool,
    /// Why, in a sentence.
    pub reason: String,
}

impl Cap {
    /// A value the disk does not change (retention, D-015).
    fn fixed(days: i64, what: &str) -> Self {
        Self {
            value: days,
            floor: days,
            ceiling: days,
            derived: false,
            reason: format!("{days} days of {what}, fixed (D-015): the disk does not change it"),
        }
    }
}

/// A project's share of the data budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectShare {
    /// Project id.
    pub project: String,
    /// Bytes of events a day.
    pub event_bytes_per_day: u64,
    /// Bytes of recordings a day.
    pub replay_bytes_per_day: u64,
    /// Bytes of one recording, on average.
    pub replay_bytes_per_recording: u64,
    /// What it takes at the floor (400 days of events, 300 recordings a
    /// day for 30 days).
    pub floor_bytes: u64,
    /// Its share of the data budget, in proportion to `floor_bytes`.
    pub share_bytes: u64,
}

/// The derived capacity: the values in force, the budget behind them and
/// why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Capacity {
    /// Whether recordings a day follow the disk; `false` when the floor
    /// applies because something could not be known (`reason` says what).
    pub derived: bool,
    /// Why the values are what they are.
    pub reason: String,
    /// Where the budget comes from: the default share or the override.
    pub budget_source: String,
    /// Size of the data volume.
    pub volume_bytes: Option<u64>,
    /// Free bytes on it.
    pub free_bytes: Option<u64>,
    /// Bytes under the data directory.
    pub used_bytes: Option<u64>,
    /// Bytes other files on the volume use.
    pub other_bytes: Option<u64>,
    /// What OpenLens may use in all.
    pub budget_bytes: Option<u64>,
    /// What events and recordings may use (half of the budget left after
    /// OpenLens's other files: the other half is the upgrade copy).
    pub data_budget_bytes: Option<u64>,
    /// What events and recordings take at the floor.
    pub floor_bytes: Option<u64>,
    /// Recordings each project keeps a day: follows the disk.
    pub replays_per_day: Cap,
    /// Days recordings are kept: fixed (D-015), listed for the record.
    pub replay_retention_days: Cap,
    /// Days events are kept: fixed (D-015), listed for the record.
    pub event_retention_days: Cap,
    /// Length of one route-stats window, seconds (60, 300 or 600; #628,
    /// D-091): advertised to the server SDKs in `x-openlens-stats-window`.
    pub stats_window: Cap,
    /// Each project's cost and share.
    pub projects: Vec<ProjectShare>,
    /// When the disk was last measured.
    pub measured_at: Option<String>,
}

impl Capacity {
    /// The floor of recordings a day, for `reason`; retention as always.
    #[must_use]
    pub fn floors(reason: &str) -> Self {
        Self {
            derived: false,
            reason: reason.to_owned(),
            budget_source: format!("default: {:.0} % of the volume", DEFAULT_SHARE * 100.0),
            volume_bytes: None,
            free_bytes: None,
            used_bytes: None,
            other_bytes: None,
            budget_bytes: None,
            data_budget_bytes: None,
            floor_bytes: None,
            replays_per_day: floor_cap(reason, false),
            replay_retention_days: Cap::fixed(REPLAY_RETENTION_DAYS, "recordings"),
            event_retention_days: Cap::fixed(EVENT_RETENTION_DAYS, "events"),
            stats_window: window::default_cap(reason),
            projects: Vec::new(),
            measured_at: None,
        }
    }
}

/// Recordings a day at the floor, for `reason`.
fn floor_cap(reason: &str, derived: bool) -> Cap {
    Cap {
        value: REPLAYS_PER_DAY_FLOOR,
        floor: REPLAYS_PER_DAY_FLOOR,
        ceiling: REPLAYS_PER_DAY_CEILING,
        derived,
        reason: format!("{REPLAYS_PER_DAY_FLOOR} recordings a day, the floor: {reason}"),
    }
}

/// The budget override: a number of bytes, or a share of the volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Override {
    /// At most this many bytes.
    Bytes(u64),
    /// This share of the volume (0.01 to 0.95), minus other files.
    Share(f64),
}

/// Read `OPENLENS_DISK_BUDGET`: `40%`, a number of bytes, or a number with
/// `kB`, `MB`, `GB`, `TB` (powers of 1000) or `KiB`, `MiB`, `GiB`, `TiB`.
///
/// # Errors
/// What is wrong, for the log and the reason.
pub fn parse_override(raw: &str) -> Result<Override, String> {
    let t = raw.trim();
    let bad = || {
        format!(
            "{OVERRIDE_VAR}={} is neither a share (1 % to 95 %) nor a number of bytes",
            t.chars().take(32).collect::<String>()
        )
    };
    if t.is_empty() || t.len() > 32 {
        return Err(bad());
    }
    if let Some(p) = t.strip_suffix('%') {
        let p: f64 = p.trim().parse().map_err(|_| bad())?;
        if !SHARE_RANGE.contains(&p) {
            return Err(bad());
        }
        return Ok(Override::Share(p / 100.0));
    }
    let digits = t
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(digits);
    let n: f64 = num.parse().map_err(|_| bad())?;
    let unit = unit.trim().to_ascii_lowercase();
    let scale: f64 = match unit.as_str() {
        "" | "b" => 1.0,
        "kb" | "k" => 1e3,
        "mb" | "m" => 1e6,
        "gb" | "g" => 1e9,
        "tb" | "t" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(bad()),
    };
    let bytes = n * scale;
    if !bytes.is_finite() || bytes < 1.0 || bytes > 1e18 {
        return Err(bad());
    }
    Ok(Override::Bytes(bytes as u64))
}

/// `bytes` for a person: `1.2 GB`, `340 MB`, `12 kB`.
#[must_use]
pub fn bytes_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e12 {
        format!("{:.1} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}

/// The values for these inputs.
#[must_use]
pub fn derive(inputs: &Inputs) -> Capacity {
    let parsed = inputs.budget_override.as_deref().map(parse_override);
    let (override_ok, override_note) = match &parsed {
        Some(Ok(o)) => (Some(*o), None),
        Some(Err(e)) => (None, Some(format!("{e}; the default applies"))),
        None => (None, None),
    };
    let budget_source = match (&override_ok, inputs.budget_override.as_deref()) {
        (Some(_), Some(raw)) => format!("{OVERRIDE_VAR}={}", raw.trim()),
        _ => match &override_note {
            Some(n) => format!(
                "default: {:.0} % of the volume ({n})",
                DEFAULT_SHARE * 100.0
            ),
            None => format!("default: {:.0} % of the volume", DEFAULT_SHARE * 100.0),
        },
    };
    let Some(volume) = inputs.volume.filter(|v| v.total > 0) else {
        let mut c = Capacity::floors("the size of the data volume cannot be read");
        c.budget_source = budget_source;
        c.measured_at = inputs.usage.as_ref().map(|u| u.measured_at.clone());
        return c;
    };
    let Some(usage) = inputs.usage.as_ref() else {
        let mut c = Capacity::floors(
            "the disk is not measured yet (the first maintenance pass measures it)",
        );
        c.budget_source = budget_source;
        c.volume_bytes = Some(volume.total);
        c.free_bytes = Some(volume.available);
        return c;
    };
    let total = volume.total as f64;
    let other = volume
        .total
        .saturating_sub(volume.available)
        .saturating_sub(usage.used);
    // What fits before the low-space guard stops writes.
    let room = (total * (1.0 - MIN_FREE_SHARE) - other as f64).max(0.0);
    let wanted = match override_ok {
        Some(Override::Bytes(b)) => b as f64,
        Some(Override::Share(s)) => total * s - other as f64,
        None => total * DEFAULT_SHARE - other as f64,
    };
    let budget = wanted.min(room).max(0.0);
    let (events_now, replays_now) = usage.projects.iter().fold((0u64, 0u64), |(e, r), p| {
        (
            e.saturating_add(p.event_bytes),
            r.saturating_add(p.replay_bytes),
        )
    });
    // OpenLens's other files: rollups, meta.db, the WAL, backups.
    let fixed = usage
        .used
        .saturating_sub(events_now)
        .saturating_sub(replays_now)
        .saturating_sub(usage.upgrade_copy) as f64;
    // Half for the data, half for its copy before an upgrade.
    let data_budget = ((budget - fixed) / 2.0).max(0.0);
    // One more recording a day, kept the fixed 30 days.
    let slot_of =
        |p: &ProjectUse| p.replay_bytes_per_recording as f64 * REPLAY_RETENTION_DAYS as f64;
    // Events as they would be with 10-minute route stats (#628): a finer
    // window is paid for below, from the room left.
    let floor_of = |p: &ProjectUse| {
        window::event_bytes_at_default(p) * EVENT_RETENTION_DAYS as f64
            + slot_of(p) * REPLAYS_PER_DAY_FLOOR as f64
    };
    let need: f64 = usage.projects.iter().map(floor_of).sum();
    // What one more recording a day in every project takes.
    let slot: f64 = usage.projects.iter().map(slot_of).sum();
    let projects = usage
        .projects
        .iter()
        .map(|p| {
            let f = floor_of(p);
            ProjectShare {
                project: p.project.clone(),
                event_bytes_per_day: p.event_bytes_per_day,
                replay_bytes_per_day: p.replay_bytes_per_day,
                replay_bytes_per_recording: p.replay_bytes_per_recording,
                floor_bytes: f as u64,
                share_bytes: if need > 0.0 {
                    (data_budget * f / need) as u64
                } else {
                    0
                },
            }
        })
        .collect();
    // The route-stats window first: its bytes must fit in the room left
    // after the floor, as extra recordings do, and come out of that room.
    let (stats_window, window_cost) = window::choose(&usage.projects, data_budget - need);
    let base = Capacity {
        derived: true,
        reason: String::new(),
        budget_source,
        volume_bytes: Some(volume.total),
        free_bytes: Some(volume.available),
        used_bytes: Some(usage.used),
        other_bytes: Some(other),
        budget_bytes: Some(budget as u64),
        data_budget_bytes: Some(data_budget as u64),
        floor_bytes: Some(need as u64),
        projects,
        measured_at: Some(usage.measured_at.clone()),
        stats_window,
        ..Capacity::floors("")
    };
    let budget_text = format!(
        "{} for events and recordings ({} free)",
        bytes_text(data_budget as u64),
        bytes_text(volume.available)
    );
    if slot <= 0.0 {
        let reason = "no recordings measured on the disk yet, so the size of one is not known";
        return Capacity {
            derived: false,
            reason: reason.to_owned(),
            replays_per_day: floor_cap(reason, false),
            ..base
        };
    }
    let extra = data_budget - need - window_cost;
    if extra <= 0.0 {
        let reason = format!(
            "{budget_text}: 400 days of events and {REPLAYS_PER_DAY_FLOOR} recordings a day for 30 days need {} (a fuller disk is stopped by the low-space guard, not by fewer recordings)",
            bytes_text(need as u64),
        );
        return Capacity {
            reason: reason.clone(),
            replays_per_day: floor_cap(&reason, true),
            ..base
        };
    }
    let more = (extra / slot).floor();
    let replays = if more.is_finite()
        && (REPLAYS_PER_DAY_FLOOR as f64 + more) < REPLAYS_PER_DAY_CEILING as f64
    {
        REPLAYS_PER_DAY_FLOOR + more as i64
    } else {
        REPLAYS_PER_DAY_CEILING
    };
    let stats_text = if window_cost > 0.0 {
        format!(
            " and {} route stats ({})",
            window::text(base.stats_window.value),
            bytes_text(window_cost as u64)
        )
    } else {
        String::new()
    };
    let reason = format!(
        "{budget_text}: after 400 days of events and {REPLAYS_PER_DAY_FLOOR} recordings a day for 30 days ({}){stats_text}, each more recording a day takes {}",
        bytes_text(need as u64),
        bytes_text(slot as u64)
    );
    Capacity {
        reason: reason.clone(),
        replays_per_day: Cap {
            value: replays,
            floor: REPLAYS_PER_DAY_FLOOR,
            ceiling: REPLAYS_PER_DAY_CEILING,
            derived: true,
            reason: if replays >= REPLAYS_PER_DAY_CEILING {
                format!("{replays} recordings a day per project (the most kept): {reason}")
            } else {
                format!("{replays} recordings a day per project: {reason}")
            },
        },
        ..base
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const GB: u64 = 1_000_000_000;

    /// A host: a volume of `total` bytes with `other` used by other files,
    /// and one project writing `events` bytes of events a day and
    /// recordings of `recording` bytes each, with the floor's worth of data
    /// on the disk.
    fn host(total: u64, other: u64, events: u64, recording: u64) -> Inputs {
        let replays_day = recording * REPLAYS_PER_DAY_FLOOR as u64;
        let event_bytes = events * EVENT_RETENTION_DAYS as u64;
        let replay_bytes = replays_day * REPLAY_RETENTION_DAYS as u64;
        let used = event_bytes + replay_bytes;
        Inputs {
            volume: Some(Volume {
                total,
                available: total.saturating_sub(used + other),
            }),
            usage: Some(Usage {
                used,
                upgrade_copy: 0,
                projects: vec![ProjectUse {
                    project: "p".into(),
                    event_bytes,
                    replay_bytes,
                    event_bytes_per_day: events,
                    replay_bytes_per_day: replays_day,
                    replay_bytes_per_recording: recording,
                    stats: None,
                }],
                measured_at: "2026-11-27T00:00:00Z".into(),
            }),
            budget_override: None,
        }
    }

    /// Retention is fixed whatever the disk (D-015).
    fn retention_fixed(c: &Capacity) -> bool {
        let fixed = |cap: &Cap, days: i64| {
            cap.value == days && cap.floor == days && cap.ceiling == days && !cap.derived
        };
        fixed(&c.event_retention_days, EVENT_RETENTION_DAYS)
            && fixed(&c.replay_retention_days, REPLAY_RETENTION_DAYS)
            && c.event_retention_days.reason.contains("D-015")
    }

    fn replays(c: &Capacity) -> i64 {
        assert!(retention_fixed(c), "{c:?}");
        c.replays_per_day.value
    }

    // 50 MB of events a day and 1 MB a recording: 29 GB at the floor.
    const EVENTS: u64 = 50_000_000;
    const RECORDING: u64 = 1_000_000;

    #[test]
    fn the_engine_and_the_retention_pass_keep_events_as_long() {
        assert_eq!(EVENT_RETENTION_DAYS, openlens_storage::EVENT_RETENTION_DAYS);
    }

    #[test]
    fn a_small_disk_keeps_the_floor() {
        let c = derive(&host(40 * GB, 5 * GB, EVENTS, RECORDING));
        assert!(c.derived);
        assert_eq!(replays(&c), REPLAYS_PER_DAY_FLOOR);
        assert!(c.reason.contains("need"), "{}", c.reason);
    }

    #[test]
    fn a_large_disk_keeps_more_recordings_up_to_the_ceiling() {
        let small = derive(&host(40 * GB, 5 * GB, EVENTS, RECORDING));
        let large = derive(&host(2_000 * GB, 5 * GB, EVENTS, RECORDING));
        assert!(replays(&large) > replays(&small));
        assert!(large.data_budget_bytes > small.data_budget_bytes);
        // The reason names the room and the cost.
        assert!(large.replays_per_day.reason.contains("GB"));
        assert!(large.replays_per_day.reason.contains("each more recording"));
        // Larger recordings, fewer of them.
        let heavy = derive(&host(2_000 * GB, 5 * GB, EVENTS, 4 * RECORDING));
        assert!(replays(&heavy) < replays(&large));
        // Never past the ceiling, however large.
        let huge = derive(&host(u64::MAX / 4, 0, 1_000, 1_000));
        assert_eq!(replays(&huge), REPLAYS_PER_DAY_CEILING);
        assert!(huge.replays_per_day.reason.contains("the most kept"));
    }

    #[test]
    fn a_filling_disk_brings_them_back_down_to_the_floor_never_below() {
        let total = 2_000 * GB;
        let mut last = derive(&host(total, 0, EVENTS, RECORDING));
        assert!(replays(&last) > REPLAYS_PER_DAY_FLOOR);
        // Other files fill the volume, step by step, to the very end.
        for step in 1..=20u64 {
            let other = total / 20 * step;
            let c = derive(&host(total, other, EVENTS, RECORDING));
            assert!(replays(&c) <= replays(&last));
            assert!(replays(&c) >= REPLAYS_PER_DAY_FLOOR);
            last = c;
        }
        assert_eq!(replays(&last), REPLAYS_PER_DAY_FLOOR);
        // OpenLens's own other files (backups) lower them the same way.
        let mut h = host(total, 0, EVENTS, RECORDING);
        let before = derive(&h);
        if let Some(u) = h.usage.as_mut() {
            u.used += 1_000 * GB;
        }
        if let Some(v) = h.volume.as_mut() {
            v.available -= 1_000 * GB;
        }
        let after = derive(&h);
        assert!(after.data_budget_bytes < before.data_budget_bytes);
        assert!(replays(&after) < replays(&before));
    }

    #[test]
    fn the_override_is_honoured_and_bounded() {
        let total = 2_000 * GB;
        let default = derive(&host(total, 0, EVENTS, RECORDING));
        let with = |raw: &str| {
            let mut h = host(total, 0, EVENTS, RECORDING);
            h.budget_override = Some(raw.into());
            derive(&h)
        };
        // A smaller share or a number of bytes: fewer recordings, and the
        // source says where the budget comes from.
        let share = with("20%");
        assert!(share.budget_bytes < default.budget_bytes);
        assert!(replays(&share) < replays(&default));
        assert!(share.budget_source.contains(OVERRIDE_VAR));
        let bytes = with("100 GB");
        assert!(bytes.budget_bytes < share.budget_bytes);
        // Above the volume, it stops where the low-space guard starts.
        let all = with("10 TB");
        let room = (total as f64 * (1.0 - MIN_FREE_SHARE)) as u64;
        assert!(all.budget_bytes.unwrap() <= room);
        assert!(all.budget_bytes > default.budget_bytes);
        // Never below the floor, however small.
        assert_eq!(replays(&with("1 MB")), REPLAYS_PER_DAY_FLOOR);
        // Not understood: the default, and the reason says so.
        for bad in ["0%", "96%", "lots", "-5", "0", "1e400"] {
            let c = with(bad);
            assert_eq!(c.budget_bytes, default.budget_bytes, "{bad}");
            assert!(c.budget_source.contains("default"), "{bad}");
            assert!(c.budget_source.contains(OVERRIDE_VAR), "{bad}");
        }
        assert_eq!(
            parse_override("2GiB"),
            Ok(Override::Bytes(2 * 1024 * 1024 * 1024))
        );
    }

    #[test]
    fn an_unknown_volume_or_no_measurement_keeps_the_floor_and_says_why() {
        let mut h = host(2_000 * GB, 0, EVENTS, RECORDING);
        h.volume = None;
        let c = derive(&h);
        assert!(!c.derived);
        assert_eq!(replays(&c), REPLAYS_PER_DAY_FLOOR);
        assert!(c.reason.contains("cannot be read"));
        assert!(c.replays_per_day.reason.contains("cannot be read"));
        let mut h = host(2_000 * GB, 0, EVENTS, RECORDING);
        h.usage = None;
        let c = derive(&h);
        assert!(!c.derived);
        assert_eq!(replays(&c), REPLAYS_PER_DAY_FLOOR);
        assert!(c.reason.contains("not measured"));
        // Events but no recording yet: the size of one is not known.
        let c = derive(&host(2_000 * GB, 0, EVENTS, 0));
        assert!(!c.derived);
        assert_eq!(replays(&c), REPLAYS_PER_DAY_FLOOR);
    }

    #[test]
    fn projects_share_the_budget_in_proportion_to_their_cost() {
        let mut h = host(2_000 * GB, 0, EVENTS, RECORDING);
        let alone = derive(&h);
        if let Some(u) = h.usage.as_mut() {
            u.projects.push(ProjectUse {
                project: "small".into(),
                event_bytes_per_day: EVENTS / 10,
                replay_bytes_per_recording: RECORDING / 10,
                ..ProjectUse::default()
            });
        }
        let c = derive(&h);
        let (big, small) = (&c.projects[0], &c.projects[1]);
        assert!(big.share_bytes > small.share_bytes);
        let total: u64 = c.projects.iter().map(|p| p.share_bytes).sum();
        assert!(total <= c.data_budget_bytes.unwrap());
        // A second project leaves fewer a day for each.
        assert!(replays(&c) < replays(&alone));
    }
}
