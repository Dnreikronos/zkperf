//! Sampled process-tree evidence, independent of report schema versions.

mod aggregate;
mod linux_io;
#[cfg(test)]
mod tests;
mod worker;

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::Observed;

pub(crate) use worker::Sampler;

const INTERVAL: Duration = Duration::from_millis(50);
const MAX_PROCESSES: usize = 4096;

/// The source and precision of a supported counter, independent of availability
/// in a particular operation. Units do not imply measurement accuracy.
#[derive(Clone, Debug, Serialize)]
pub struct ResourceCapability {
    pub source: &'static str,
    pub unit: &'static str,
    pub semantics: &'static str,
    pub resolution: Observed<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResourceCapabilities {
    pub wall_time: Observed<ResourceCapability>,
    pub cpu_time: Observed<ResourceCapability>,
    pub resident_memory: Observed<ResourceCapability>,
    pub io: Observed<ResourceCapability>,
    pub gpu: Observed<ResourceCapability>,
    pub linux_high_water: Observed<ResourceCapability>,
}

impl ResourceCapabilities {
    fn for_platform(platform: &str) -> Self {
        let (cpu, memory, io) = match platform {
            "linux" => (
                "proc_pid_stat",
                "proc_pid_stat_rss",
                "proc_pid_task_tid_io_storage_bytes",
            ),
            "macos" => (
                "proc_pidinfo_taskinfo",
                "proc_pidinfo_resident_size",
                "proc_pid_rusage_disk_bytes",
            ),
            "windows" => (
                "get_process_times",
                "process_working_set",
                "get_process_io_counters_transfer_bytes",
            ),
            _ => ("", "", ""),
        };
        let capability = |source: &'static str, unit, semantics| {
            if source.is_empty() {
                Observed::gap(
                    "unsupported_platform",
                    "No process counter backend for this host.",
                )
            } else {
                ResourceCapability {
                    source,
                    unit,
                    semantics,
                    resolution: Observed::gap(
                        "not_exposed",
                        "Counter or clock resolution is not exposed by this backend.",
                    ),
                }
                .into()
            }
        };
        Self {
            wall_time: capability(
                "std_time_instant",
                "nanosecond",
                "operation_phase_duration_ns; monotonic; excludes_cleanup; watchdog_poll_interval_2ms",
            ),
            cpu_time: capability(
                cpu,
                "nanosecond",
                "sum_of_last_observed_process_lifetime_counters; backend_quantized_to_milliseconds",
            ),
            resident_memory: capability(
                memory,
                "byte",
                "maximum_observed_sweep_sum; non_atomic; shared_pages_can_repeat",
            ),
            io: capability(
                io,
                "byte",
                if platform == "windows" {
                    "sum_of_last_observed_transfer_bytes; includes_file_device_and_pipe_io"
                } else {
                    "sum_of_last_observed_storage_bytes; excludes_cached_logical_io"
                },
            ),
            gpu: Observed::gap(
                "not_implemented",
                "GPU providers may add namespaced extensions with scope and precision.",
            ),
            linux_high_water: Observed::gap(
                "not_implemented",
                "Kernel high-water providers are separate from sampled tree RSS.",
            ),
        }
    }
}

/// Bounded sampling diagnostics. Collection cost is elapsed worker time, not CPU
/// usage, and includes scans discarded at the phase boundary.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CollectionDiagnostics {
    pub samples: u64,
    pub observed_processes: usize,
    pub missing_process_observations: u64,
    pub counter_regressions: u64,
    pub io_observed_tasks: usize,
    pub io_failed_reads: u64,
    pub io_task_limit_reached: bool,
    pub collection_duration_ns: u64,
    pub max_collection_duration_ns: u64,
    pub max_sample_gap_ns: u64,
    pub last_sample_offset_ns: u64,
    pub process_limit_reached: bool,
    pub counter_saturated: bool,
}

/// Resource values are partial observations, even when available. Wall duration
/// is the owning `OperationRecord::phase_duration_ns`; it is not duplicated here.
#[derive(Clone, Debug, Serialize)]
pub struct ResourceEvidence {
    pub capture_version: &'static str,
    pub backend: &'static str,
    pub platform: &'static str,
    pub capabilities: ResourceCapabilities,
    pub cpu_time_ns: Observed<u64>,
    pub sampled_peak_rss_bytes: Observed<u64>,
    pub io_read_bytes: Observed<u64>,
    pub io_written_bytes: Observed<u64>,
    pub minimum_sample_interval_ns: u64,
    pub target_worker_duty_cycle_percent: u8,
    pub max_tracked_processes: usize,
    pub max_tracked_io_tasks: usize,
    pub collection: CollectionDiagnostics,
    pub limitations: Vec<&'static str>,
    pub extensions: BTreeMap<String, Value>,
}

impl ResourceEvidence {
    pub(crate) fn unavailable(code: &str, message: &str) -> Self {
        Self {
            capture_version: "1.0.0",
            backend: "sysinfo_0_36",
            platform: std::env::consts::OS,
            capabilities: ResourceCapabilities::for_platform(std::env::consts::OS),
            cpu_time_ns: Observed::gap(code, message),
            sampled_peak_rss_bytes: Observed::gap(code, message),
            io_read_bytes: Observed::gap(code, message),
            io_written_bytes: Observed::gap(code, message),
            minimum_sample_interval_ns: nanos(INTERVAL),
            target_worker_duty_cycle_percent: 5,
            max_tracked_processes: MAX_PROCESSES,
            max_tracked_io_tasks: MAX_PROCESSES,
            collection: CollectionDiagnostics::default(),
            limitations: vec![
                "Partial sampled process tree: short-lived or already-reparented children and final counter increments may be missed.",
                "RSS is a non-atomic sampled sum, not a kernel high-water mark; shared pages may be counted more than once.",
                "PID identity uses start time in whole seconds; reuse entirely between samples within the same second is indistinguishable.",
                "Zero counters may mean denied or unavailable reads; all-zero metrics are unavailable and partial read failures can undercount totals.",
                "CPU counters are quantized to milliseconds; storage units and sampling intervals do not imply kernel or clock accuracy.",
                "Linux I/O sums observed thread lifetimes to avoid waited-child roll-ups; short-lived threads and final increments can be missed.",
                "Collector contention can perturb wall time; the 5% duty-cycle target is amortized and does not bound one OS call or process enumeration.",
            ],
            extensions: BTreeMap::new(),
        }
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn positive(value: u64) -> Observed<u64> {
    if value == 0 {
        Observed::gap(
            "no_positive_observation",
            "No positive counter was observed; zero and unavailable cannot be distinguished.",
        )
    } else {
        value.into()
    }
}
