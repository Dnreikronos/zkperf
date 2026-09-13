use super::*;

fn invocation(fixture: &Fixture, mode: &str) -> AdapterInvocation {
    let mut invocation = fixture.invocation(mode);
    invocation.arguments[0] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/resources.py")
        .into_os_string();
    invocation.request["timeout"]["limit_ns"] = 10_000_000_000_u64.into();
    invocation
}

#[test]
fn descendants_contribute_cpu_memory_and_io_even_after_exit() {
    let mut fixture = Fixture::new();
    let (result, _) = fixture.run(
        &invocation(&fixture, "success"),
        &CancellationToken::default(),
    );
    assert_eq!(
        result.record.outcome,
        OperationOutcome::Success,
        "{:?}",
        result.record
    );
    let usage = &result.record.resources;
    assert!(usage.collection.observed_processes >= 3, "{usage:?}");
    assert!(
        usage
            .cpu_time_ns
            .value()
            .is_some_and(|value| *value >= 100_000_000),
        "{usage:?}"
    );
    assert!(
        usage
            .sampled_peak_rss_bytes
            .value()
            .is_some_and(|value| *value >= 64 * 1024 * 1024),
        "{usage:?}"
    );
    assert!(
        usage
            .io_written_bytes
            .value()
            .is_some_and(|value| *value >= 8 * 1024 * 1024),
        "{usage:?}"
    );
    if cfg!(target_os = "linux") {
        // One 8 MiB write must not become 16/24 MiB as each ancestor waits.
        assert!(
            *usage.io_written_bytes.value().unwrap() < 12 * 1024 * 1024,
            "{usage:?}"
        );
        assert!(usage.collection.io_observed_tasks >= 4);
    }
    assert!(usage.collection.samples >= 2);
    assert!(usage.collection.collection_duration_ns > 0);
    assert!(
        usage.collection.samples
            <= result.record.phase_duration_ns / usage.minimum_sample_interval_ns + 1
    );
    assert!(usage.collection.last_sample_offset_ns <= result.record.phase_duration_ns);
    assert!(!usage.limitations.is_empty());
}

#[test]
fn failures_and_timeouts_retain_resource_evidence() {
    for (mode, limit, expected) in [
        (
            "failure",
            10_000_000_000_u64,
            OperationOutcome::ProcessFailed,
        ),
        ("success", 1_500_000_000, OperationOutcome::TimedOut),
    ] {
        let mut fixture = Fixture::new();
        let mut invocation = invocation(&fixture, mode);
        invocation.request["timeout"]["limit_ns"] = limit.into();
        let (result, _) = fixture.run(&invocation, &CancellationToken::default());
        assert_eq!(result.record.outcome, expected, "{:?}", result.record);
        assert!(
            result.record.resources.collection.observed_processes >= 3,
            "{:?}",
            result.record.resources
        );
        assert!(
            result.record.resources.collection.last_sample_offset_ns
                <= result.record.phase_duration_ns
        );
    }
}

#[test]
fn cancellation_grace_allocations_are_excluded_from_resource_samples() {
    let mut fixture = Fixture::new();
    let mut invocation = invocation(&fixture, "grace");
    invocation.graceful_cancellation = true;
    let token = CancellationToken::default();
    let cancel = token.clone();
    let pid = fixture.root.join("pid");
    let trigger = std::thread::spawn(move || {
        let start = Instant::now();
        while !pid.exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(500));
        cancel.cancel();
    });
    let (result, _) = fixture.run(&invocation, &token);
    trigger.join().unwrap();
    assert_eq!(result.record.outcome, OperationOutcome::Cancelled);
    assert!(result.record.cleanup_duration_ns >= 300_000_000);
    let usage = &result.record.resources;
    assert!(
        usage
            .sampled_peak_rss_bytes
            .value()
            .is_some_and(|value| *value < 128 * 1024 * 1024),
        "{usage:?}"
    );
    assert!(usage.collection.last_sample_offset_ns <= result.record.phase_duration_ns);
}

#[test]
fn spawn_failure_and_pre_spawn_cancellation_report_unavailable_resources() {
    for cancelled in [false, true] {
        let mut fixture = Fixture::new();
        let mut invocation = invocation(&fixture, "success");
        invocation.executable = fixture.root.join("nonexistent-adapter");
        let token = CancellationToken::default();
        if cancelled {
            token.cancel();
        }
        let (result, _) = fixture.run(&invocation, &token);
        let usage = serde_json::to_value(result.record.resources).unwrap();
        assert_eq!(usage["cpu_time_ns"]["reason"]["code"], "not_spawned");
        assert_eq!(usage["collection"]["samples"], 0);
    }
}
