use serde::Deserialize;
use serde_json::{Value, json};
use zkperf_core::{BenchmarkReport, Count, Ratio, RunMetadata, SummaryValue, UnitInterval};

const SUCCESSFUL: &str = include_str!("../../../examples/reports/successful.json");
const FAILED: &str = include_str!("../../../examples/reports/failed.json");
const TIMED_OUT: &str = include_str!("../../../examples/reports/timed-out.json");
const PARTIALLY_SUPPORTED: &str =
    include_str!("../../../examples/reports/partially-supported.json");
const INVALID_REPORTS: &str = include_str!("../../../tests/fixtures/invalid-reports.json");

fn successful_report() -> Value {
    serde_json::from_str(SUCCESSFUL).unwrap()
}

fn failed_report() -> Value {
    serde_json::from_str(FAILED).unwrap()
}

fn rejects(report: Value) -> bool {
    serde_json::from_value::<BenchmarkReport>(report).is_err()
}

#[test]
fn rejects_opaque_or_null_normative_metadata() {
    for path in ["benchmark", "engine", "environment"] {
        let mut report = successful_report();
        report[path] = Value::Null;
        assert!(rejects(report), "{path} accepted null");
    }

    for path in ["suite", "policy"] {
        let mut report = successful_report();
        report["run"][path] = Value::Null;
        assert!(rejects(report), "run.{path} accepted null");
    }
}

#[test]
fn rejects_phase_incompatible_correctness() {
    let mut report = successful_report();
    report["measurements"][0]["samples"][0]["correctness"]["verification_verdict"] =
        Value::String("rejected".to_owned());
    assert!(rejects(report));
}

#[test]
fn rejects_statistics_disconnected_from_observations() {
    let mut report = successful_report();
    report["measurements"][0]["statistics"]["status_counts"]["success"] = Value::from(999);
    report["measurements"][0]["statistics"]["minimum"] = Value::from(1);
    assert!(rejects(report));
}

#[test]
fn rejects_integer_summaries_with_tolerance_sized_errors() {
    for (observation, incorrect_summary) in [
        (1_000_000_000_u64, 1_000_000_001_u64),
        (9_007_199_254_740_992_u64, 9_007_199_254_740_993_u64),
    ] {
        let mut report = successful_report();
        for sample in &mut report["measurements"][1]["samples"].as_array_mut().unwrap()[1..] {
            sample["value"] = Value::from(observation);
        }
        let statistics = &mut report["measurements"][1]["statistics"];
        for field in ["minimum", "maximum", "median", "mean"] {
            statistics[field] = Value::from(incorrect_summary);
        }
        statistics["standard_deviation"] = Value::from(0);
        statistics["percentiles"]["p50"] = Value::from(incorrect_summary);
        statistics["percentiles"]["p95"] = Value::from(incorrect_summary);

        assert!(
            rejects(report),
            "accepted {incorrect_summary} for exact observation {observation}"
        );
    }
}

#[test]
fn rejects_decimal_bounds_before_f64_rounding() {
    assert!(serde_json::from_str::<Ratio>("-1e-400").is_err());
    assert!(serde_json::from_str::<Ratio>("1.00000000000000001").is_err());
    assert!(serde_json::from_str::<SummaryValue<Count>>("-1e-400").is_err());
    assert!(serde_json::from_str::<UnitInterval>("-1e-400").is_err());
    assert!(serde_json::from_str::<UnitInterval>("1.00000000000000001").is_err());

    let mut report = successful_report();
    report["measurements"][0]["statistics"]["rates"]["failure"] =
        serde_json::from_str("-1e-400").unwrap();
    assert!(rejects(report));
}

#[test]
fn rejects_missing_proof_size_observation() {
    let mut report = successful_report();
    report["measurements"].as_array_mut().unwrap().remove(1);
    assert!(rejects(report));
}

#[test]
fn rejects_disconnected_schedule_and_attempt_graph() {
    let mut duplicate_schedule_position = successful_report();
    duplicate_schedule_position["run"]["planned_order"][1]["position"] = Value::from(0);
    assert!(rejects(duplicate_schedule_position));

    let mut unknown_schedule_position = successful_report();
    unknown_schedule_position["measurements"][0]["samples"][1]["schedule_position"] =
        Value::from(99);
    assert!(rejects(unknown_schedule_position));

    let mut duplicate_sample_id = successful_report();
    let existing_id = duplicate_sample_id["measurements"][0]["samples"][1]["id"].clone();
    duplicate_sample_id["measurements"][1]["samples"][1]["id"] = existing_id;
    assert!(rejects(duplicate_sample_id));

    let mut divergent_attempt_metadata = successful_report();
    divergent_attempt_metadata["measurements"][1]["samples"][1]["started_at"] =
        Value::String("2026-07-25T12:00:00.500Z".to_owned());
    assert!(rejects(divergent_attempt_metadata));

    let mut wrong_rate = successful_report();
    wrong_rate["measurements"][0]["statistics"]["rates"]["failure"] = Value::from(0.5);
    assert!(rejects(wrong_rate));
}

#[test]
fn rejects_replacement_self_references() {
    let mut report = successful_report();
    invalidate_attempt(&mut report, 0, Some(0));
    assert!(rejects(report));
}

#[test]
fn rejects_replacement_cycles() {
    let mut report = successful_report();
    invalidate_attempt(&mut report, 0, Some(1));
    invalidate_attempt(&mut report, 1, Some(0));
    report["measurements"][0]["policy"]["measured_trial_count"] = Value::from(2);

    set_remaining_sample_statistics(&mut report);

    assert!(rejects(report));
}

#[test]
fn accepts_replacement_lineage_terminating_at_original_attempt() {
    let mut report = successful_report();
    invalidate_attempt(&mut report, 0, None);
    invalidate_attempt(&mut report, 1, Some(0));
    set_replacement_target(&mut report, 2, 1);
    report["measurements"][0]["policy"]["measured_trial_count"] = Value::from(1);

    set_remaining_sample_statistics(&mut report);

    assert!(!rejects(report));
}

fn set_remaining_sample_statistics(report: &mut Value) {
    set_two_sample_statistics(
        &mut report["measurements"][0]["statistics"],
        &json!({
            "minimum": 90_000,
            "maximum": 110_000,
            "median": 100_000,
            "mean": 100_000,
            "standard_deviation": 14_142.135_623_730_95,
            "p50": 100_000,
            "p95": 109_000
        }),
    );
    set_two_sample_statistics(
        &mut report["measurements"][1]["statistics"],
        &json!({
            "minimum": 992,
            "maximum": 1_008,
            "median": 1_000,
            "mean": 1_000,
            "standard_deviation": 11.313_708_498_984_761,
            "p50": 1_000,
            "p95": 1_007.2
        }),
    );
}

#[test]
fn rejects_unknown_sample_properties() {
    let mut report = successful_report();
    report["measurements"][0]["samples"][0]["unknown"] = Value::Bool(true);
    assert!(rejects(report));
}

#[test]
fn rejects_unknown_timing_properties() {
    let mut report = successful_report();
    report["measurements"][0]["samples"][0]["timing"]["unknown"] = Value::Bool(true);
    assert!(rejects(report));
}

#[test]
fn rejects_explicit_null_for_optional_properties() {
    let mut extensions = successful_report();
    extensions["extensions"] = Value::Null;
    assert!(rejects(extensions));

    let mut artifact_attempt = successful_report();
    artifact_attempt["artifacts"][0]["attempt_id"] = Value::Null;
    assert!(rejects(artifact_attempt));

    let mut firmware = successful_report();
    firmware["environment"]["host"]["firmware_or_microcode"] = Value::Null;
    assert!(rejects(firmware));

    let mut statistic_mean = successful_report();
    statistic_mean["measurements"][0]["statistics"]["mean"] = Value::Null;
    assert!(rejects(statistic_mean));

    let mut replacement = successful_report();
    replacement["measurements"][0]["samples"][0]["replacement_attempt_id"] = Value::Null;
    assert!(rejects(replacement));

    let mut diagnostic_timing = failed_report();
    diagnostic_timing["measurements"][0]["samples"][0]["diagnostic_timing"] = Value::Null;
    assert!(rejects(diagnostic_timing));

    let mut warning_path = failed_report();
    warning_path["warnings"][0]["path"] = Value::Null;
    assert!(rejects(warning_path));
}

#[test]
fn rejects_empty_planned_order() {
    let mut report = failed_report();
    report["run"]["planned_order"] = json!([]);
    assert!(serde_json::from_value::<RunMetadata>(report["run"].clone()).is_err());
}

#[test]
fn rejects_invalid_schema_constrained_leaves() {
    let mut artifact_uri = successful_report();
    artifact_uri["artifacts"][0]["uri"] = Value::String("http://[invalid".to_owned());
    assert!(rejects(artifact_uri));

    let mut warning_pointer = failed_report();
    warning_pointer["warnings"][0]["path"] = Value::String("not-a-pointer".to_owned());
    assert!(rejects(warning_pointer));

    let mut extension_namespace = successful_report();
    extension_namespace["extensions"] = json!({"bad": true});
    assert!(rejects(extension_namespace));
}

#[derive(Deserialize)]
struct InvalidCase {
    name: String,
    base: String,
    operations: Vec<PatchOperation>,
}

#[derive(Deserialize)]
struct PatchOperation {
    op: String,
    path: String,
    value: Option<Value>,
}

#[test]
fn rejects_every_published_negative_fixture() {
    let cases: Vec<InvalidCase> = serde_json::from_str(INVALID_REPORTS).unwrap();
    for case in cases {
        let source = match case.base.as_str() {
            "successful.json" => SUCCESSFUL,
            "failed.json" => FAILED,
            "timed-out.json" => TIMED_OUT,
            "partially-supported.json" => PARTIALLY_SUPPORTED,
            base => panic!("unknown fixture base {base}"),
        };
        let mut report: Value = serde_json::from_str(source).unwrap();
        for operation in case.operations {
            apply_patch(&mut report, operation);
        }
        assert!(rejects(report), "negative fixture accepted: {}", case.name);
    }
}

fn invalidate_attempt(report: &mut Value, sample_index: usize, target_index: Option<usize>) {
    let target_id =
        target_index.map(|index| report["measurements"][0]["samples"][index]["attempt_id"].clone());
    for measurement in report["measurements"].as_array_mut().unwrap() {
        let sample = measurement["samples"][sample_index]
            .as_object_mut()
            .unwrap();
        sample.insert("status".to_owned(), Value::String("invalid".to_owned()));
        sample.insert(
            "reason".to_owned(),
            json!({
                "code": "invalid_attempt",
                "message": "Harness interference invalidated this attempt."
            }),
        );
        if let Some(target_id) = &target_id {
            sample.insert("replacement_attempt_id".to_owned(), target_id.clone());
        } else {
            sample.remove("replacement_attempt_id");
        }
        for field in ["timing", "correctness", "value"] {
            sample.remove(field);
        }

        let statistics = &mut measurement["statistics"];
        statistics["status_counts"]["success"] =
            Value::from(4_u64.saturating_sub(sample_index as u64 + 1));
        statistics["status_counts"]["invalid"] = Value::from(sample_index as u64 + 1);
    }
    report["status"] = json!({
        "outcome": "invalid",
        "reason": {
            "code": "invalid_attempt",
            "message": "One or more attempts were invalid."
        }
    });
}

fn set_replacement_target(report: &mut Value, sample_index: usize, target_index: usize) {
    let target_id = report["measurements"][0]["samples"][target_index]["attempt_id"].clone();
    for measurement in report["measurements"].as_array_mut().unwrap() {
        measurement["samples"][sample_index]["replacement_attempt_id"] = target_id.clone();
    }
}

fn set_two_sample_statistics(statistics: &mut Value, summaries: &Value) {
    statistics["sample_count"] = Value::from(2);
    for field in ["minimum", "maximum", "median", "mean", "standard_deviation"] {
        statistics[field] = summaries[field].clone();
    }
    statistics["percentiles"]["p50"] = summaries["p50"].clone();
    statistics["percentiles"]["p95"] = summaries["p95"].clone();
    statistics["included_attempt_ids"] = json!([
        "20000000-0000-4000-8000-000000000003",
        "20000000-0000-4000-8000-000000000004"
    ]);
}

fn apply_patch(document: &mut Value, operation: PatchOperation) {
    let tokens: Vec<_> = operation
        .path
        .split('/')
        .skip(1)
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect();
    let (last, parents) = tokens.split_last().unwrap();
    let mut parent = document;
    for token in parents {
        parent = match parent {
            Value::Object(object) => object.get_mut(token).unwrap(),
            Value::Array(array) => &mut array[token.parse::<usize>().unwrap()],
            _ => panic!("patch path traverses scalar: {}", operation.path),
        };
    }

    match (operation.op.as_str(), parent) {
        ("add" | "replace", Value::Object(object)) => {
            object.insert(last.clone(), operation.value.unwrap());
        }
        ("add", Value::Array(array)) => {
            array.insert(last.parse().unwrap(), operation.value.unwrap());
        }
        ("replace", Value::Array(array)) => {
            array[last.parse::<usize>().unwrap()] = operation.value.unwrap();
        }
        ("remove", Value::Object(object)) => {
            object.remove(last).unwrap();
        }
        ("remove", Value::Array(array)) => {
            array.remove(last.parse().unwrap());
        }
        (op, _) => panic!("unsupported patch operation {op}"),
    }
}
