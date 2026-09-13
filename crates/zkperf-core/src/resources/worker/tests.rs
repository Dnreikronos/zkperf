use super::*;

fn sampled(cpu_ns: u64) -> Aggregate {
    let mut aggregate = Aggregate::new(1, 10);
    aggregate.sample(
        &[Process {
            identity: Identity {
                pid: 1,
                started: 10,
            },
            parent: None,
            counters: Counters {
                cpu_ns,
                rss: cpu_ns,
                read: cpu_ns * 2,
                written: cpu_ns * 3,
            },
        }],
        Duration::from_millis(1),
    );
    aggregate
}

#[test]
fn sweep_counters_must_finish_before_both_stop_and_deadline() {
    for (stop_ms, limit_ms, accepted) in [
        (None, 25, true),
        (Some(10), 25, false),
        (Some(20), 25, false),
        (Some(30), 25, true),
        (None, 20, false),
        (Some(30), 15, false),
    ] {
        let start = Instant::now();
        let mut aggregate = sampled(10);
        let candidate = sampled(1_000);
        let boundary = Duration::from_millis(stop_ms.unwrap_or(limit_ms).min(limit_ms));
        complete_sweep(
            &mut aggregate,
            candidate,
            start,
            start + Duration::from_millis(5),
            start + Duration::from_millis(20),
            boundary,
        );
        let evidence = aggregate.finish(boundary);
        let expected = if accepted { 1_000 } else { 10 };
        assert_eq!(evidence.cpu_time_ns.value(), Some(&expected));
        assert_eq!(evidence.io_written_bytes.value(), Some(&(expected * 3)));
        let collection = evidence.collection.value().unwrap();
        assert_eq!(collection.collection_duration_ns, 15_000_000);
        assert_eq!(collection.max_collection_duration_ns, 15_000_000);
    }
}

#[test]
fn published_stop_discards_a_sweep_even_before_its_wakeup_arrives() {
    let (_send, receive) = mpsc::channel::<()>();
    let stop = StopBoundary::default();
    let stopped = stop.request();
    let start = stopped.checked_sub(Duration::from_millis(20)).unwrap();
    let mut aggregate = sampled(10);
    assert_eq!(receive.try_recv(), Err(mpsc::TryRecvError::Empty));
    let boundary = stop.at().unwrap().duration_since(start);
    complete_sweep(
        &mut aggregate,
        sampled(1_000),
        start,
        stopped.checked_sub(Duration::from_millis(5)).unwrap(),
        stopped + Duration::from_millis(5),
        boundary,
    );
    let evidence = aggregate.finish(boundary);
    assert_eq!(evidence.cpu_time_ns.value(), Some(&10));
    assert_eq!(evidence.io_written_bytes.value(), Some(&30));
}

#[test]
fn repeated_stop_calls_return_the_published_phase_boundary() {
    let (send, receive) = mpsc::channel();
    let mut sampler = Sampler {
        stop: Some(send),
        boundary: Arc::new(StopBoundary::default()),
        worker: None,
        ready: Arc::new(AtomicBool::new(true)),
    };
    let stopped = sampler.stop();
    assert_eq!(sampler.boundary.at(), Some(stopped));
    assert_eq!(receive.try_recv(), Ok(()));
    assert_eq!(sampler.stop(), stopped);
    assert_eq!(receive.try_recv(), Err(mpsc::TryRecvError::Disconnected));
}

#[test]
fn missing_or_panicked_worker_does_not_fabricate_zero_diagnostics() {
    let panicked = thread::spawn(|| {
        let mut aggregate = Aggregate::new(1, 10);
        aggregate.sample(&[], INTERVAL);
        assert_eq!(aggregate.evidence.collection.value().unwrap().samples, 1);
        panic!("collector failed after sampling");
    });
    for worker in [None, Some(panicked)] {
        let sampler = Sampler {
            stop: None,
            boundary: Arc::new(StopBoundary::default()),
            worker,
            ready: Arc::new(AtomicBool::new(true)),
        };
        let evidence = serde_json::to_value(sampler.finish()).unwrap();
        assert_eq!(evidence["collection"]["availability"], "unavailable");
        assert_eq!(evidence["collection"]["reason"]["code"], "collector_failed");
        assert!(evidence["collection"].get("samples").is_none());
    }
}
