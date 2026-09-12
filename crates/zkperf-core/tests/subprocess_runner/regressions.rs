use super::Fixture;
use zkperf_core::{CancellationToken, OperationOutcome};

#[test]
fn invalid_diagnostic_artifacts_preserve_adapter_failure_status() {
    for (status, expected) in [
        ("unsupported", OperationOutcome::Unsupported),
        ("error", OperationOutcome::AdapterError),
        ("success", OperationOutcome::ArtifactError),
    ] {
        for problem in ["digest", "missing"] {
            let fixture = Fixture::new();
            let mode = format!("artifact-diagnostic-{status}-{problem}");
            let (result, _) =
                fixture.run(&fixture.invocation(&mode), &CancellationToken::default());
            assert_eq!(
                result.record.outcome, expected,
                "{mode}: {:?}",
                result.record
            );
            assert_eq!(result.response.unwrap()["status"], status);
            assert!(!result.record.errors.is_empty(), "{mode}");
            assert!(result.artifacts.is_empty(), "{mode}");
        }
    }
}
