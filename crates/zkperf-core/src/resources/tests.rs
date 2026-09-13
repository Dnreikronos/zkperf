use std::time::Duration;

use super::aggregate::{Aggregate, Counters, Identity, Process};
use super::{
    INTERVAL, MAX_PROCESSES, ResourceCapabilities, ResourceEvidence, worker::wait_duration,
};

fn process(pid: u32, parent: u32, started: u64, cpu_ns: u64, rss: u64) -> Process {
    Process {
        identity: Identity { pid, started },
        parent: Some(parent),
        counters: Counters {
            cpu_ns,
            rss,
            read: cpu_ns * 2,
            written: cpu_ns * 3,
        },
    }
}

#[test]
fn tree_totals_retain_exited_children_without_summing_individual_rss_peaks() {
    let mut aggregate = Aggregate::new(1, 10);
    aggregate.sample(
        &[
            process(1, 0, 10, 10, 100),
            process(2, 1, 10, 20, 200),
            process(3, 2, 11, 30, 300),
            process(9, 0, 10, 999, 999),
        ],
        INTERVAL,
    );
    aggregate.sample(&[process(1, 0, 10, 40, 400)], INTERVAL * 2);
    let result = aggregate.finish(INTERVAL * 3);
    assert_eq!(result.cpu_time_ns.value(), Some(&90));
    assert_eq!(result.io_read_bytes.value(), Some(&180));
    assert_eq!(result.io_written_bytes.value(), Some(&270));
    assert_eq!(result.sampled_peak_rss_bytes.value(), Some(&600));
    assert_eq!(result.collection.observed_processes, 3);
    assert_eq!(result.collection.missing_process_observations, 2);
}

#[test]
fn reparented_known_children_remain_tracked_but_reused_pids_do_not_inherit_membership() {
    let mut aggregate = Aggregate::new(1, 10);
    aggregate.sample(
        &[process(1, 0, 10, 10, 100), process(2, 1, 11, 20, 200)],
        INTERVAL,
    );
    aggregate.sample(
        &[
            process(1, 0, 12, 999, 999),
            process(2, 0, 11, 40, 400),
            process(3, 1, 12, 999, 999),
            process(4, 2, 12, 50, 500),
        ],
        INTERVAL * 2,
    );
    let result = aggregate.finish(INTERVAL * 2);
    assert_eq!(result.cpu_time_ns.value(), Some(&100));
    assert_eq!(result.collection.observed_processes, 3);
}

#[test]
fn retired_identity_is_not_resurrected_even_with_same_second_start_time() {
    let mut aggregate = Aggregate::new(1, 10);
    let root = process(1, 0, 10, 10, 100);
    aggregate.sample(&[root, process(2, 1, 10, 20, 200)], INTERVAL);
    aggregate.sample(&[root], INTERVAL * 2);
    aggregate.sample(&[root, process(2, 1, 10, 999, 999)], INTERVAL * 3);
    assert_eq!(
        aggregate.finish(INTERVAL * 3).cpu_time_ns.value(),
        Some(&30)
    );
}

#[test]
fn a_new_child_with_a_reused_pid_gets_separate_lifetime_counters() {
    let mut aggregate = Aggregate::new(1, 10);
    let root = process(1, 0, 10, 10, 100);
    aggregate.sample(&[root, process(2, 1, 10, 20, 200)], INTERVAL);
    aggregate.sample(&[root, process(2, 1, 11, 30, 300)], INTERVAL * 2);
    let result = aggregate.finish(INTERVAL * 2);
    assert_eq!(result.cpu_time_ns.value(), Some(&60));
    assert_eq!(result.collection.observed_processes, 3);
}

#[test]
fn missed_root_and_changed_identities_cannot_seed_an_unrelated_tree() {
    for first in [
        vec![],
        vec![process(1, 0, 9, 999, 999)],
        vec![process(1, 0, 11, 999, 999)],
    ] {
        let mut aggregate = Aggregate::new(1, 10);
        aggregate.sample(&first, INTERVAL);
        aggregate.sample(&[process(1, 0, 11, 999, 999)], INTERVAL * 2);
        let result = aggregate.finish(INTERVAL * 2);
        assert_eq!(result.collection.observed_processes, 0);
        assert!(result.cpu_time_ns.value().is_none());
    }
}

#[test]
fn counter_regressions_do_not_wrap_or_subtract_prior_usage() {
    let mut aggregate = Aggregate::new(1, 10);
    aggregate.sample(&[process(1, 0, 10, 100, 100)], INTERVAL);
    aggregate.sample(&[process(1, 0, 10, 1, 0)], INTERVAL * 2);
    let result = aggregate.finish(INTERVAL * 2);
    assert_eq!(result.cpu_time_ns.value(), Some(&100));
    assert_eq!(result.collection.counter_regressions, 1);
}

#[test]
fn process_history_is_capped_and_arithmetic_saturation_is_disclosed() {
    let mut aggregate = Aggregate::new(1, 10);
    let mut processes: Vec<_> = (1..=u32::try_from(MAX_PROCESSES + 3).unwrap())
        .map(|pid| process(pid, 1, 10, 10, 100))
        .collect();
    for process in &mut processes {
        process.counters.cpu_ns = u64::MAX;
    }
    aggregate.sample(&processes, INTERVAL);
    let result = aggregate.finish(INTERVAL);
    assert_eq!(result.collection.observed_processes, MAX_PROCESSES);
    assert!(result.collection.process_limit_reached);
    assert!(result.collection.counter_saturated);
}

#[test]
fn unsupported_platforms_and_missing_metrics_have_reasons() {
    let capabilities = ResourceCapabilities::for_platform("unsupported-test-host");
    assert!(capabilities.wall_time.value().is_some());
    let value = serde_json::to_value(capabilities).unwrap();
    for metric in [
        "cpu_time",
        "resident_memory",
        "io",
        "gpu",
        "linux_high_water",
    ] {
        assert_eq!(value[metric]["availability"], "unavailable");
        assert!(value[metric]["reason"]["message"].is_string());
    }
    for platform in ["linux", "macos", "windows"] {
        assert!(
            ResourceCapabilities::for_platform(platform)
                .cpu_time
                .value()
                .is_some()
        );
    }
    let evidence = ResourceEvidence::unavailable("not_spawned", "Adapter was not spawned.");
    assert_eq!(evidence.collection.samples, 0);
    assert!(evidence.cpu_time_ns.value().is_none());
    assert!(evidence.extensions.is_empty());
}

#[test]
fn all_zero_counters_are_not_claimed_as_measured_zero() {
    let mut aggregate = Aggregate::new(1, 10);
    aggregate.sample(&[process(1, 0, 10, 0, 0)], INTERVAL);
    let result = aggregate.finish(INTERVAL * 2);
    assert!(result.cpu_time_ns.value().is_none());
    assert!(result.sampled_peak_rss_bytes.value().is_none());
    assert!(result.io_read_bytes.value().is_none());
    assert!(result.io_written_bytes.value().is_none());
}

#[test]
fn scheduling_backs_off_after_slow_samples_and_never_catches_up() {
    for cost in [
        Duration::ZERO,
        Duration::from_millis(1),
        INTERVAL,
        Duration::from_secs(2),
    ] {
        let wait = wait_duration(cost);
        assert!(wait >= INTERVAL);
        assert!(wait >= cost * 19);
    }
}
