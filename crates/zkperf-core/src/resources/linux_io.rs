use std::collections::BTreeMap;

use cap_std::{ambient_authority, fs::Dir};

use super::aggregate::Identity;
use super::{CollectionDiagnostics, MAX_PROCESSES};

/// /proc/PID/io includes waited-for children. Per-thread io does not, so retain
/// thread lifetimes instead of summing the process-level roll-ups.
#[derive(Clone, Default)]
pub(super) struct LinuxIo {
    retained: BTreeMap<(Identity, u32, u64), (u64, u64)>,
}

impl LinuxIo {
    pub fn sample(&mut self, processes: &[Identity], stats: &mut CollectionDiagnostics) {
        let clock = process_clock();
        let mut visited = 0;
        for process in processes {
            let Some(tasks) = open_tasks(*process, clock) else {
                stats.io_failed_reads += 1;
                continue;
            };
            let Ok(entries) = tasks.entries() else {
                stats.io_failed_reads += 1;
                continue;
            };
            for entry in entries {
                if visited == MAX_PROCESSES {
                    stats.io_task_limit_reached = true;
                    return;
                }
                visited += 1;
                let observation = entry.ok().and_then(|entry| {
                    let tid = entry.file_name().to_str()?.parse().ok()?;
                    let directory = tasks.open_dir(entry.file_name()).ok()?;
                    let before = start_ticks(&directory.read_to_string("stat").ok()?)?;
                    let io = parse_io(&directory.read_to_string("io").ok()?)?;
                    let after = start_ticks(&directory.read_to_string("stat").ok()?)?;
                    (before == after).then_some((tid, before, io))
                });
                if let Some((tid, started, io)) = observation {
                    self.observe((*process, tid, started), io, stats);
                } else {
                    stats.io_failed_reads += 1;
                }
            }
        }
    }

    fn observe(
        &mut self,
        key: (Identity, u32, u64),
        io: (u64, u64),
        stats: &mut CollectionDiagnostics,
    ) {
        if !self.retained.contains_key(&key) && self.retained.len() == MAX_PROCESSES {
            stats.io_task_limit_reached = true;
            return;
        }
        let old = self.retained.entry(key).or_default();
        if io.0 < old.0 || io.1 < old.1 {
            stats.counter_regressions += 1;
        }
        old.0 = old.0.max(io.0);
        old.1 = old.1.max(io.1);
        stats.io_observed_tasks = self.retained.len();
    }

    pub fn total(&self, stats: &mut CollectionDiagnostics) -> (u64, u64) {
        self.retained.values().fold((0_u64, 0_u64), |total, io| {
            let sum = (total.0.checked_add(io.0), total.1.checked_add(io.1));
            stats.counter_saturated |= sum.0.is_none() || sum.1.is_none();
            (sum.0.unwrap_or(u64::MAX), sum.1.unwrap_or(u64::MAX))
        })
    }
}

fn process_clock() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        let boot_time = sysinfo::System::boot_time();
        let ticks_per_second = rustix::param::clock_ticks_per_second();
        (boot_time != 0 && ticks_per_second != 0).then_some((boot_time, ticks_per_second))
    }
    #[cfg(not(target_os = "linux"))]
    None
}

fn open_tasks(process: Identity, clock: Option<(u64, u64)>) -> Option<Dir> {
    let (boot_time, ticks_per_second) = clock?;
    let directory =
        Dir::open_ambient_dir(format!("/proc/{}", process.pid), ambient_authority()).ok()?;
    let ticks = start_ticks(&directory.read_to_string("stat").ok()?)?;
    // Match sysinfo's whole Unix seconds before attaching tasks to its snapshot.
    let started = ticks
        .checked_div(ticks_per_second)?
        .checked_add(boot_time)?;
    if started != process.started {
        return None;
    }
    // Stay on the validated proc handle even if the numeric PID is reused.
    directory.open_dir("task").ok()
}

fn start_ticks(stat: &str) -> Option<u64> {
    // comm may contain spaces and parentheses; field 22 follows its final ')'.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn parse_io(contents: &str) -> Option<(u64, u64)> {
    let field = |key| {
        contents.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name == key).then(|| value.trim().parse().ok()).flatten()
        })
    };
    Some((field("read_bytes")?, field("write_bytes")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    fn live_identity() -> Identity {
        let pid = sysinfo::Pid::from_u32(std::process::id());
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().without_tasks(),
        );
        Identity {
            pid: pid.as_u32(),
            started: system.process(pid).unwrap().start_time(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_process_identity_rejects_live_tasks_and_preserves_prior_counters() {
        let live = live_identity();
        for started in [1, live.started - 1] {
            let stale = Identity { started, ..live };
            let mut io = LinuxIo::default();
            let mut stats = CollectionDiagnostics::default();
            io.sample(&[stale], &mut stats);
            assert_eq!(stats.io_failed_reads, 1);
            assert_eq!(stats.io_observed_tasks, 0);
            assert_eq!(io.total(&mut stats), (0, 0));

            io.observe((stale, stale.pid, 100), (5, 6), &mut stats);
            io.sample(&[stale], &mut stats);
            assert_eq!(stats.io_failed_reads, 2);
            assert_eq!(stats.io_observed_tasks, 1);
            assert_eq!(io.total(&mut stats), (5, 6));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn matching_process_identity_collects_live_task_counters() {
        let live = live_identity();
        let mut io = LinuxIo::default();
        let mut stats = CollectionDiagnostics::default();
        io.sample(&[live], &mut stats);
        assert!(stats.io_observed_tasks > 0);
        assert!(io.retained.keys().all(|(identity, _, _)| *identity == live));
        assert!(io.retained.keys().any(|(_, tid, _)| *tid == live.pid));
    }

    #[test]
    fn thread_io_does_not_inherit_reaped_children_and_reused_tids_are_separate() {
        let mut io = LinuxIo::default();
        let mut stats = CollectionDiagnostics::default();
        let parent = Identity {
            pid: 1,
            started: 10,
        };
        let child = Identity {
            pid: 2,
            started: 11,
        };
        io.observe((parent, 1, 100), (10, 20), &mut stats);
        io.observe((child, 2, 110), (100, 200), &mut stats);
        // Parent's per-thread counter remains its own after wait(), while a new
        // thread with the same numeric TID begins a distinct counter lifetime.
        io.observe((parent, 1, 100), (11, 22), &mut stats);
        io.observe((child, 2, 120), (5, 6), &mut stats);
        assert_eq!(io.total(&mut stats), (116, 228));
    }

    #[test]
    fn missing_and_malformed_proc_fields_are_not_zero() {
        assert_eq!(parse_io("read_bytes: 0\nwrite_bytes: 12\n"), Some((0, 12)));
        assert_eq!(parse_io("read_bytes: 1\n"), None);
        assert_eq!(parse_io("read_bytes: nope\nwrite_bytes: 12\n"), None);
        let stat = format!("123 (name ) with spaces) S {} 12345 0", "0 ".repeat(18));
        assert_eq!(start_ticks(&stat), Some(12345));
        assert_eq!(start_ticks("process vanished"), None);
    }

    #[test]
    fn disappeared_task_directories_preserve_prior_observations_and_record_the_gap() {
        let mut io = LinuxIo::default();
        let mut stats = CollectionDiagnostics::default();
        let process = Identity {
            pid: u32::MAX,
            started: 10,
        };
        io.observe((process, 1, 100), (5, 6), &mut stats);
        io.sample(&[process], &mut stats);
        assert_eq!(stats.io_failed_reads, 1);
        assert_eq!(io.total(&mut stats), (5, 6));
    }

    #[test]
    fn thread_history_has_a_fixed_cap() {
        let mut io = LinuxIo::default();
        let mut stats = CollectionDiagnostics::default();
        let process = Identity {
            pid: 1,
            started: 10,
        };
        for tid in 0..=u32::try_from(MAX_PROCESSES).unwrap() {
            io.observe((process, tid, 100), (1, 1), &mut stats);
        }
        assert_eq!(stats.io_observed_tasks, MAX_PROCESSES);
        assert!(stats.io_task_limit_reached);
        assert_eq!(io.total(&mut stats), (4096, 4096));
    }
}
