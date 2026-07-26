use std::collections::BTreeMap;
use std::num::NonZeroU64;

use serde_json::Map;
use zkperf_core::{
    Architecture, BenchmarkJob, BenchmarkMetadata, BenchmarkMetadataParts, BenchmarkReportV1,
    BenchmarkReportV1Parts, BuildCacheState, ByteSize, CanonicalInput, CanonicalInputParts,
    ClockMetadata, Commitment, CommitmentPolicy, CompatibilityMetadata, CompilerMetadata,
    Concurrency, ConcurrencyMode, Correctness, CpuMetadata, DurationOutcome, EngineMetadata,
    EngineMetadataParts, EnvironmentMetadata, EnvironmentMetadataParts, ExecutionMode,
    FailedProofCorrectness, GuestMetadata, GuestMetadataParts, HostMetadata, HostMetadataParts,
    ImplementationLane, InputVisibility, Lifecycle, LifecycleParts, LifecycleProfile, Measurement,
    MeasurementError, Nanoseconds, NonEmptyString, OperatingSystemMetadata, PercentileMethod,
    Phase, ProofCorrectness, ProofMetadata, ProofMetadataParts, Reason, ReportStatus,
    ResourceLimit, Resources, ResourcesParts, RunMetadata, RunMetadataParts, RunPolicy,
    RunPolicyParts, ScalarMeasurement, SemanticVersion, SetupCacheState, Sha256Digest, Slug,
    StandardPhase, SuiteMetadata, SuiteMetadataParts, Timestamp, Timing, ToolMetadata,
    WorkloadMetadata,
};

fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value).unwrap()
}

fn slug(value: &str) -> Slug {
    Slug::new(value).unwrap()
}

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(byte.to_string().repeat(64)).unwrap()
}

fn nonzero(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}

fn reason() -> Reason {
    Reason::new(slug("unsupported"), text("not supported"))
}

fn build_run(engine_id: &Slug) -> RunMetadata {
    let suite = SuiteMetadata::new(SuiteMetadataParts {
        id: slug("suite"),
        revision: text("suite-r1"),
        manifest_digest: digest('a'),
    });
    let policy = RunPolicy::new(RunPolicyParts {
        ordering_algorithm: text("seeded"),
        seed: 7,
        concurrency: Concurrency::new(ConcurrencyMode::Isolated, nonzero(1)).unwrap(),
        retry_policy: text("none"),
        invalidation_policy: text("replace"),
        outlier_rule: text("none"),
        percentile_method: PercentileMethod::Linear,
    });
    RunMetadata::new(RunMetadataParts {
        id: "10000000-0000-4000-8000-000000000000".parse().unwrap(),
        harness_revision: text("harness-r1"),
        suite,
        started_at: Timestamp::parse("2026-07-26T12:00:00Z").unwrap(),
        finished_at: Timestamp::parse("2026-07-26T12:00:01Z").unwrap(),
        policy,
        planned_order: vec![BenchmarkJob::new(
            0,
            engine_id.clone(),
            Phase::standard(StandardPhase::Proving),
            0,
            false,
        )],
    })
    .unwrap()
}

fn build_benchmark() -> BenchmarkMetadata {
    BenchmarkMetadata::new(BenchmarkMetadataParts {
        case_id: slug("case"),
        workload: WorkloadMetadata::new(text("workload-r1"), digest('b')),
        implementation_lane: ImplementationLane::Portable,
        canonical_input: CanonicalInput::new(CanonicalInputParts {
            byte_length: ByteSize::new(4),
            digest: digest('c'),
            visibility: InputVisibility::Public,
            preprocessing_policy: text("canonical"),
        }),
        expected_output_digest: digest('d'),
        statement: text("compute"),
        commitment_policy: CommitmentPolicy::new(Commitment::None, Commitment::None),
        security_target_bits: nonzero(100),
        lifecycle: Lifecycle::new(LifecycleParts {
            profile: LifecycleProfile::Cold,
            preparation_pins: vec![],
            build_cache_state: BuildCacheState::Cold,
            setup_cache_state: SetupCacheState::Cold,
            setup_reuse_count: 0,
        }),
        parameters: Map::new(),
    })
}

fn build_engine(engine_id: Slug) -> EngineMetadata {
    let guest = GuestMetadata::new(GuestMetadataParts {
        source_revision: text("guest-r1"),
        toolchain: ToolMetadata::new(text("toolchain"), text("1.0")),
        compiler: CompilerMetadata::new(text("compiler"), text("1.0"), vec![]),
        artifact_digest: digest('e'),
        optimizations: vec![],
    });
    let proof = ProofMetadata::new(ProofMetadataParts {
        raw_format: text("raw"),
        final_format: text("final"),
        transformation_pipeline: vec![],
        verifier: ToolMetadata::new(text("verifier"), text("1.0")),
        parameters_version: text("params-r1"),
        parameters_digest: digest('f'),
    });
    EngineMetadata::new(EngineMetadataParts {
        id: engine_id,
        name: text("Engine"),
        version: text("1.0"),
        adapter_revision: text("adapter-r1"),
        proof_system: text("proof-system"),
        backend: text("backend"),
        configured_security_bits: nonzero(128),
        trust_profile: text("default"),
        guest,
        proof,
        configuration: Map::new(),
    })
}

fn build_environment() -> EnvironmentMetadata {
    let host = HostMetadata::new(HostMetadataParts {
        machine_id: text("machine"),
        architecture: Architecture::X86_64,
        cpu: CpuMetadata::new(text("cpu"), text("1"), nonzero(4), nonzero(8)),
        ram_bytes: nonzero(1_024),
        accelerators: vec![],
        storage: text("ssd"),
        operating_system: OperatingSystemMetadata::new(text("os"), text("1.0"), text("kernel")),
        firmware_or_microcode: None,
    });
    let resources = Resources::new(ResourcesParts {
        power_profile: text("performance"),
        cpu_affinity: vec![0],
        cpu_limit: ResourceLimit::Unlimited,
        memory_limit_bytes: ResourceLimit::Unlimited,
        worker_count: nonzero(1),
        accelerator_allocation: vec![],
        environment_variables: BTreeMap::new(),
        execution_mode: ExecutionMode::Local,
        network_access: false,
        remote: None,
    })
    .unwrap();
    EnvironmentMetadata::new(EnvironmentMetadataParts {
        host,
        resources,
        clock: ClockMetadata::new(text("monotonic"), nonzero(1)),
    })
}

#[test]
fn external_producer_builds_report_without_json_deserialization() {
    let engine_id = slug("engine");
    let run = build_run(&engine_id);
    let benchmark = build_benchmark();
    let engine = build_engine(engine_id);
    let environment = build_environment();

    let unsupported = reason();
    let report = BenchmarkReportV1::new(BenchmarkReportV1Parts {
        report_id: "20000000-0000-4000-8000-000000000000".parse().unwrap(),
        contract: CompatibilityMetadata::new(SemanticVersion::parse("1.0.0").unwrap()),
        run,
        benchmark,
        engine,
        environment,
        measurements: vec![Measurement::proving_key_size(
            ScalarMeasurement::unavailable(None, unsupported.clone()),
        )],
        artifacts: vec![],
        warnings: vec![],
        status: ReportStatus::PartiallySupported {
            reason: unsupported,
        },
        extensions: None,
    })
    .unwrap();

    assert_eq!(
        serde_json::to_value(report).unwrap()["schema_version"],
        "1.0.0"
    );
}

#[test]
fn proof_size_constructor_rejects_missing_phase() {
    let measurement = ScalarMeasurement::unavailable(None, reason());
    assert!(matches!(
        Measurement::proof_size(measurement),
        Err(MeasurementError::MissingProofSizePhase)
    ));
}

#[test]
fn external_producer_builds_accepted_and_rejected_proof_evidence() {
    let accepted = DurationOutcome::Success {
        timing: Timing::new(Nanoseconds::new(10), Nanoseconds::new(20)).unwrap(),
        correctness: Some(Correctness::AcceptedProof(ProofCorrectness::new(
            digest('d'),
            Some(digest('1')),
            Some(digest('2')),
        ))),
    };
    let rejected = DurationOutcome::Failed {
        error_code: slug("verification_failed"),
        reason: Reason::new(slug("verification_failed"), text("proof was rejected")),
        diagnostic_timing: None,
        correctness: Some(Correctness::RejectedProof(FailedProofCorrectness::new(
            Some(digest('d')),
            Some(digest('1')),
            Some(digest('2')),
        ))),
    };

    assert_eq!(
        serde_json::to_value(accepted).unwrap()["correctness"]["verification_verdict"],
        "accepted"
    );
    assert_eq!(
        serde_json::to_value(rejected).unwrap()["correctness"]["verification_verdict"],
        "rejected"
    );
}
