use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use super::{MAX_PROCESSES, ResourceEvidence, nanos, positive};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Identity {
    pub pid: u32,
    pub started: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Counters {
    pub cpu_ns: u64,
    pub rss: u64,
    pub read: u64,
    pub written: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Process {
    pub identity: Identity,
    pub parent: Option<u32>,
    pub counters: Counters,
}

#[derive(Clone)]
struct Retained {
    counters: Counters,
    active: bool,
}

#[derive(Clone)]
pub(super) struct Aggregate {
    root: Identity,
    seeded: bool,
    retained: BTreeMap<Identity, Retained>,
    task_io: Option<super::linux_io::LinuxIo>,
    pub evidence: ResourceEvidence,
}

impl Aggregate {
    pub fn new(pid: u32, started: u64) -> Self {
        Self {
            root: Identity { pid, started },
            seeded: false,
            retained: BTreeMap::new(),
            task_io: None,
            evidence: ResourceEvidence::unavailable(
                "not_observed",
                "No process sample was captured.",
            ),
        }
    }

    pub fn sample(&mut self, processes: &[Process], offset: Duration) {
        let current: BTreeMap<_, _> = processes.iter().map(|p| (p.identity.pid, p)).collect();
        for (id, retained) in &mut self.retained {
            if retained.active && current.get(&id.pid).is_none_or(|p| p.identity != *id) {
                retained.active = false;
                self.evidence.collection_mut().missing_process_observations += 1;
            }
        }
        if !self.seeded {
            self.seeded = true;
            if let Some(root) = current.get(&self.root.pid) {
                if root.identity == self.root && root.identity.started != 0 {
                    self.insert(root);
                }
            }
        }
        let mut children: BTreeMap<u32, Vec<&Process>> = BTreeMap::new();
        for process in processes {
            if let Some(parent) = process.parent {
                children.entry(parent).or_default().push(process);
            }
        }
        let mut queue: VecDeque<_> = self
            .retained
            .iter()
            .filter(|(_, p)| p.active)
            .map(|(id, _)| *id)
            .collect();
        while let Some(parent) = queue.pop_front() {
            if let Some(descendants) = children.get(&parent.pid) {
                for child in descendants {
                    if child.identity.started >= parent.started && self.insert(child) {
                        queue.push_back(child.identity);
                    }
                }
            }
        }
        let mut rss = 0_u64;
        for (identity, retained) in &mut self.retained {
            if !retained.active {
                continue;
            }
            let counters = current[&identity.pid].counters;
            if counters.cpu_ns < retained.counters.cpu_ns
                || counters.read < retained.counters.read
                || counters.written < retained.counters.written
            {
                self.evidence.collection_mut().counter_regressions += 1;
            }
            retained.counters.cpu_ns = retained.counters.cpu_ns.max(counters.cpu_ns);
            retained.counters.read = retained.counters.read.max(counters.read);
            retained.counters.written = retained.counters.written.max(counters.written);
            rss = sum(
                rss,
                counters.rss,
                &mut self.evidence.collection_mut().counter_saturated,
            );
        }
        let old_peak = self
            .evidence
            .sampled_peak_rss_bytes
            .value()
            .copied()
            .unwrap_or(0);
        if !self.retained.is_empty() {
            self.evidence.sampled_peak_rss_bytes = positive(old_peak.max(rss), "resident memory");
        }
        let stats = self.evidence.collection_mut();
        let offset = nanos(offset);
        stats.max_sample_gap_ns = stats
            .max_sample_gap_ns
            .max(offset.saturating_sub(stats.last_sample_offset_ns));
        stats.last_sample_offset_ns = offset;
        stats.samples += 1;
        stats.observed_processes = self.retained.len();
    }

    pub fn sample_linux_io(&mut self) {
        let processes: Vec<_> = self
            .retained
            .iter()
            .filter(|(_, retained)| retained.active)
            .map(|(id, _)| *id)
            .collect();
        self.task_io
            .get_or_insert_default()
            .sample(&processes, self.evidence.collection_mut());
    }

    fn insert(&mut self, process: &Process) -> bool {
        if self.retained.contains_key(&process.identity) {
            return false;
        }
        if self.retained.len() == MAX_PROCESSES {
            self.evidence.collection_mut().process_limit_reached = true;
            return false;
        }
        self.retained.insert(
            process.identity,
            Retained {
                counters: Counters::default(),
                active: true,
            },
        );
        true
    }

    pub fn finish(mut self, elapsed: Duration) -> ResourceEvidence {
        let mut total = Counters::default();
        let saturated = &mut self.evidence.collection_mut().counter_saturated;
        for retained in self.retained.values() {
            total.cpu_ns = sum(total.cpu_ns, retained.counters.cpu_ns, saturated);
            total.read = sum(total.read, retained.counters.read, saturated);
            total.written = sum(total.written, retained.counters.written, saturated);
        }
        if !self.retained.is_empty() {
            self.evidence.cpu_time_ns = positive(total.cpu_ns, "CPU time");
            self.evidence.io_read_bytes = positive(total.read, "read I/O");
            self.evidence.io_written_bytes = positive(total.written, "write I/O");
            if let Some(io) = &self.task_io {
                let (read, written) = io.total(self.evidence.collection_mut());
                self.evidence.io_read_bytes = positive(read, "read I/O");
                self.evidence.io_written_bytes = positive(written, "write I/O");
            }
        }
        let stats = self.evidence.collection_mut();
        stats.max_sample_gap_ns = stats
            .max_sample_gap_ns
            .max(nanos(elapsed).saturating_sub(stats.last_sample_offset_ns));
        self.evidence
    }
}

fn sum(left: u64, right: u64, saturated: &mut bool) -> u64 {
    if let Some(value) = left.checked_add(right) {
        value
    } else {
        *saturated = true;
        u64::MAX
    }
}
