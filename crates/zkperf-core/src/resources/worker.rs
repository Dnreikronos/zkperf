#[cfg(test)]
mod tests;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use super::aggregate::{Aggregate, Counters, Identity, Process};
use super::{INTERVAL, ResourceEvidence, nanos};

pub(crate) struct Sampler {
    stop: Option<Sender<()>>,
    boundary: Arc<StopBoundary>,
    worker: Option<JoinHandle<ResourceEvidence>>,
    ready: Arc<AtomicBool>,
}

#[derive(Default)]
struct StopBoundary(Mutex<Option<Instant>>);

impl StopBoundary {
    fn request(&self) -> Instant {
        // Capture and publish together, so a delayed wakeup cannot hide an
        // earlier stop timestamp from the sweep commit check.
        *self.0.lock().unwrap().get_or_insert_with(Instant::now)
    }

    fn at(&self) -> Option<Instant> {
        *self.0.lock().unwrap()
    }
}

struct Bootstrap(Arc<AtomicBool>);

impl Drop for Bootstrap {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Sampler {
    pub fn start(pid: u32, start: Instant, limit: Duration) -> Self {
        let (send, receive) = mpsc::channel();
        let boundary = Arc::new(StopBoundary::default());
        let worker_boundary = Arc::clone(&boundary);
        let ready = Arc::new(AtomicBool::new(false));
        let bootstrap = Bootstrap(Arc::clone(&ready));
        let worker = thread::Builder::new()
            .name("zkperf-resources".into())
            .spawn(move || collect(pid, start, limit, &worker_boundary, &receive, bootstrap))
            .ok();
        if worker.is_none() {
            ready.store(true, Ordering::Release);
        }
        Self {
            stop: Some(send),
            boundary,
            worker,
            ready,
        }
    }

    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn pending_for_test() -> Self {
        Self {
            stop: None,
            boundary: Arc::new(StopBoundary::default()),
            worker: None,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stop(&mut self) -> Instant {
        let stopped = self.boundary.request();
        if let Some(send) = self.stop.take() {
            let _ = send.send(());
        }
        stopped
    }

    pub fn finish(mut self) -> ResourceEvidence {
        self.stop();
        self.worker
            .take()
            .and_then(|worker| worker.join().ok())
            .unwrap_or_else(ResourceEvidence::collector_failed)
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            drop(worker.join());
        }
    }
}

fn collect(
    pid: u32,
    start: Instant,
    limit: Duration,
    stop: &StopBoundary,
    wake: &Receiver<()>,
    bootstrap: Bootstrap,
) -> ResourceEvidence {
    if !matches!(std::env::consts::OS, "linux" | "macos" | "windows") {
        return ResourceEvidence::unavailable(
            "unsupported_platform",
            "No process counter backend for this host.",
        );
    }
    let setup = Instant::now();
    let root = identify(pid);
    drop(bootstrap);
    let setup_cost = nanos(setup.elapsed());
    let Some(root) = root else {
        let mut evidence = ResourceEvidence::unavailable(
            "not_observed",
            "The adapter's process identity could not be read before reap.",
        );
        evidence.collection_mut().collection_duration_ns = setup_cost;
        evidence.collection_mut().max_collection_duration_ns = setup_cost;
        return evidence;
    };
    let mut aggregate = Aggregate::new(root.pid, root.started);
    aggregate.evidence.collection_mut().collection_duration_ns = setup_cost;
    aggregate
        .evidence
        .collection_mut()
        .max_collection_duration_ns = setup_cost;
    loop {
        if let Some(stopped) = stop.at() {
            return aggregate.finish(stopped.duration_since(start).min(limit));
        }
        if start.elapsed() >= limit {
            return aggregate.finish(limit);
        }
        let sweep = Instant::now();
        let mut candidate = aggregate.clone();
        let processes = snapshot();
        candidate.sample(&processes, start.elapsed());
        if cfg!(target_os = "linux") {
            candidate.sample_linux_io();
        }
        let completed = Instant::now();
        let cost = completed.duration_since(sweep);
        let stopped = stop.at();
        let boundary = stopped.map_or(limit, |at| at.duration_since(start).min(limit));
        complete_sweep(&mut aggregate, candidate, start, sweep, completed, boundary);
        if let Some(stopped) = stopped {
            return aggregate.finish(stopped.duration_since(start).min(limit));
        }
        if completed.duration_since(start) >= limit {
            return aggregate.finish(limit);
        }
        match wake.recv_timeout(wait_duration(cost).min(limit.saturating_sub(start.elapsed()))) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let elapsed = stop.at().unwrap_or_else(Instant::now).duration_since(start);
                return aggregate.finish(elapsed.min(limit));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn complete_sweep(
    aggregate: &mut Aggregate,
    mut candidate: Aggregate,
    start: Instant,
    sweep: Instant,
    completed: Instant,
    boundary: Duration,
) {
    let cost = nanos(completed.duration_since(sweep));
    let stats = aggregate.evidence.collection_mut();
    stats.collection_duration_ns = stats.collection_duration_ns.saturating_add(cost);
    stats.max_collection_duration_ns = stats.max_collection_duration_ns.max(cost);
    candidate.evidence.collection_mut().collection_duration_ns = stats.collection_duration_ns;
    candidate
        .evidence
        .collection_mut()
        .max_collection_duration_ns = stats.max_collection_duration_ns;
    if completed.duration_since(start) < boundary {
        *aggregate = candidate;
    }
}

pub(super) fn wait_duration(cost: Duration) -> Duration {
    INTERVAL.max(cost.saturating_mul(19))
}

fn snapshot() -> Vec<Process> {
    // Rebuilding avoids cached counters or identities surviving a failed refresh.
    // Exclude Linux task entries: process CPU counters already include threads.
    let mut system = System::new();
    let mut refresh = ProcessRefreshKind::nothing()
        .without_tasks()
        .with_cpu()
        .with_memory();
    if !cfg!(target_os = "linux") {
        refresh = refresh.with_disk_usage();
    }
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
    system
        .processes()
        .values()
        .map(|process| {
            let io = process.disk_usage();
            Process {
                identity: Identity {
                    pid: process.pid().as_u32(),
                    started: process.start_time(),
                },
                parent: process.parent().map(sysinfo::Pid::as_u32),
                counters: Counters {
                    cpu_ns: process.accumulated_cpu_time().saturating_mul(1_000_000),
                    rss: process.memory(),
                    read: io.total_read_bytes,
                    written: io.total_written_bytes,
                },
            }
        })
        .collect()
}

fn identify(pid: u32) -> Option<Identity> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().without_tasks(),
    );
    let process = system.process(pid)?;
    (process.start_time() != 0).then_some(Identity {
        pid: pid.as_u32(),
        started: process.start_time(),
    })
}
