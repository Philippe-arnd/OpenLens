//! Query memory and parallelism from the machine, not constants (#444).
//!
//! The query memory pool was a fixed 768 MiB and every query used two
//! partitions, whatever the host: memory beyond 1 GiB was never used, and
//! under 768 MiB the kernel killed the process before DataFusion spilled.
//! Both now follow the container (D-033). There is no setting: when the
//! detection is wrong, the detection is fixed.

use std::path::Path;

use crate::Limits;

const MIB: u64 = 1024 * 1024;
/// Kept out of the pool for the rest of the process: HTTP, ingestion
/// buffers, caches, the allocator's own overhead.
const RESERVED: u64 = 256 * MIB;
/// Smallest pool, even on a tiny host: below it no board can run.
const FLOOR: u64 = 256 * MIB;

/// The memory this process may use, in bytes: the cgroup limit (v2, then
/// v1), capped by physical memory, or physical memory alone. `None` when
/// none of them can be read.
pub(crate) fn memory_limit() -> Option<u64> {
    let physical = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| meminfo_total(&s));
    let own = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|s| own_cgroup(&s));
    let v2 = own.and_then(|path| {
        // In a container the process's cgroup is the root of what it
        // sees; under systemd (`MemoryMax=`) it is a path below the root,
        // and any ancestor may set a tighter limit.
        ancestors(&path)
            .filter_map(|dir| read_limit(&Path::new("/sys/fs/cgroup").join(dir).join("memory.max")))
            .min()
    });
    let cgroup =
        v2.or_else(|| read_limit(Path::new("/sys/fs/cgroup/memory/memory.limit_in_bytes")));
    effective(cgroup, physical)
}

/// The cgroup v2 path of this process, from `/proc/self/cgroup`
/// (`0::/system.slice/openlens.service`), without its leading `/`.
pub(crate) fn own_cgroup(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(|p| p.trim().trim_start_matches('/').to_owned())
}

/// `a/b/c`, then `a/b`, `a` and `` (the root).
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(path);
    std::iter::from_fn(move || {
        let cur = next?;
        next = if cur.is_empty() {
            None
        } else {
            Some(
                cur.rfind('/')
                    .map_or("", |i| cur.get(..i).unwrap_or_default()),
            )
        };
        Some(cur)
    })
}

/// The query pool for a process allowed `limit` bytes.
pub(crate) fn pool_for(limit: Option<u64>) -> u64 {
    match limit {
        Some(l) => l.saturating_sub(RESERVED).max(FLOOR),
        // Unknown host: what the pool always was.
        None => 768 * MIB,
    }
}

/// Threads for queries: the CPUs this process may use (quota included),
/// from 1 to 8.
pub(crate) fn query_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 8))
}

fn read_limit(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| parse_limit(&s))
}

/// A cgroup memory limit file: a number of bytes, or `max` (no limit).
/// cgroup v1 writes a huge number instead of `max`; [`effective`] caps it.
pub(crate) fn parse_limit(text: &str) -> Option<u64> {
    let t = text.trim();
    if t == "max" {
        return None;
    }
    t.parse::<u64>().ok().filter(|&n| n > 0)
}

/// `MemTotal` of `/proc/meminfo`, in bytes.
pub(crate) fn meminfo_total(text: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let rest = l.strip_prefix("MemTotal:")?;
        let kib = rest.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()?;
        kib.checked_mul(1024)
    })
}

/// The cgroup limit unless it is above physical memory (no real limit).
pub(crate) fn effective(cgroup: Option<u64>, physical: Option<u64>) -> Option<u64> {
    match (cgroup, physical) {
        (Some(c), Some(p)) => Some(c.min(p)),
        (Some(c), None) => Some(c),
        (None, p) => p,
    }
}

/// A byte count in MiB, for the startup log.
pub(crate) fn mib(bytes: u64) -> u64 {
    bytes / MIB
}

impl Limits {
    /// The defaults, with the memory pool sized to this host (#444, D-033):
    /// the container's memory limit (cgroup v2 or v1, else physical
    /// memory) minus 256 MiB, at least 256 MiB. 768 MiB under a 1 GiB
    /// limit, as before. Logs what it chose.
    pub fn from_host() -> Self {
        let limit = memory_limit();
        let pool = pool_for(limit);
        tracing::info!(
            query_memory_mib = mib(pool),
            memory_limit_mib = limit.map(mib),
            query_threads = query_threads(),
            "query limits from the host"
        );
        Self {
            memory: usize::try_from(pool).unwrap_or(usize::MAX),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn cgroup_files_never_panic_and_max_means_no_limit() {
        assert_eq!(parse_limit("1073741824\n"), Some(1024 * MIB));
        assert_eq!(parse_limit("max\n"), None);
        assert_eq!(parse_limit(""), None);
        assert_eq!(parse_limit("garbage"), None);
        assert_eq!(read_limit(Path::new("/nonexistent/memory.max")), None);
    }

    #[test]
    fn a_systemd_unit_limit_is_found_from_the_process_cgroup() {
        assert_eq!(
            own_cgroup("0::/system.slice/openlens.service\n").as_deref(),
            Some("system.slice/openlens.service")
        );
        // In a container, the process sees its cgroup as the root.
        assert_eq!(own_cgroup("0::/\n").as_deref(), Some(""));
        assert_eq!(own_cgroup("12:memory:/x\n"), None);
        let all: Vec<&str> = ancestors("system.slice/openlens.service").collect();
        assert_eq!(all, ["system.slice/openlens.service", "system.slice", ""]);
    }

    #[test]
    fn the_pool_follows_the_container() {
        // Under the default Helm limit, the pool is what it always was.
        assert_eq!(pool_for(Some(1024 * MIB)), 768 * MIB);
        assert!(pool_for(Some(2048 * MIB)) > pool_for(Some(1024 * MIB)));
        // A tiny container still gets a usable pool, and never one larger
        // than the container itself when it is above the floor.
        assert_eq!(pool_for(Some(128 * MIB)), FLOOR);
        assert!(pool_for(Some(512 * MIB)) < 512 * MIB);
    }

    #[test]
    fn a_cgroup_above_physical_memory_is_no_limit() {
        // cgroup v1 writes "no limit" as a huge number.
        assert_eq!(
            effective(Some(9_223_372_036_854_771_712), Some(8 * 1024 * MIB)),
            Some(8 * 1024 * MIB)
        );
        assert_eq!(effective(None, Some(4 * 1024 * MIB)), Some(4 * 1024 * MIB));
        assert_eq!(effective(None, None), None);
        assert_eq!(
            meminfo_total("MemTotal:       16303428 kB\nMemFree: 1 kB\n"),
            Some(16_303_428 * 1024)
        );
        assert_eq!(meminfo_total("nothing"), None);
    }
}
