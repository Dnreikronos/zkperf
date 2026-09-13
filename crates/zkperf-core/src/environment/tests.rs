use std::collections::BTreeMap;

use serde_json::json;

use super::{EnvironmentCapture, safe_environment};

#[test]
fn environment_allowlist_validates_values_and_excludes_secrets() {
    let source = BTreeMap::from([
        ("RAYON_NUM_THREADS".into(), "8".into()),
        ("OMP_NUM_THREADS".into(), "secret-in-allowed-name".into()),
        ("OMP_DYNAMIC".into(), "FALSE".into()),
        ("API_TOKEN".into(), "secret-token".into()),
        ("PATH".into(), "private-path".into()),
        ("RUSTFLAGS".into(), "private-flags".into()),
    ]);
    let value = serde_json::to_value(safe_environment(&source)).unwrap();
    assert_eq!(value["RAYON_NUM_THREADS"], "8");
    assert_eq!(value["OMP_DYNAMIC"], "FALSE");
    assert_eq!(value["OMP_NUM_THREADS"]["availability"], "unavailable");
    for secret in [
        "secret-in-allowed-name",
        "secret-token",
        "private-path",
        "private-flags",
        "API_TOKEN",
    ] {
        assert!(!value.to_string().contains(secret));
    }
    for invalid in ["", "0", "-1", "+1", "4294967296", "8 secret"] {
        let value = safe_environment(&BTreeMap::from([(
            "RAYON_NUM_THREADS".into(),
            invalid.into(),
        )]));
        assert!(matches!(
            value["RAYON_NUM_THREADS"],
            crate::Observed::Unavailable(_)
        ));
    }
}

#[test]
fn host_capture_and_hash_round_trip_without_volatile_data() {
    let first = EnvironmentCapture::collect(&BTreeMap::new());
    let second = EnvironmentCapture::collect(&BTreeMap::new());
    assert_eq!(first, second);
    assert_eq!(first.digest(), second.digest());
    let value = serde_json::to_value(&first).unwrap();
    let restored: EnvironmentCapture = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(restored.digest(), first.digest());
    let mut wrong_version = value.clone();
    wrong_version["capture_version"] = json!("2.0.0");
    assert!(serde_json::from_value::<EnvironmentCapture>(wrong_version).is_err());
    assert_eq!(value["harness"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        value["harness"]["rustc"]
            .as_str()
            .unwrap()
            .starts_with("rustc ")
    );
    assert_eq!(value["host"]["machine_id"]["reason"]["code"], "excluded");
    assert_eq!(
        value["clock"]["resolution_ns"]["availability"],
        "unavailable"
    );
    if sysinfo::IS_SUPPORTED_SYSTEM {
        assert!(value["host"]["ram_bytes"].as_u64().unwrap() > 0);
        assert!(value["host"]["cpu"]["logical_cores"].as_u64().unwrap() > 0);
    }
    let mut changed = value;
    changed["environment_variables"] = json!({"RAYON_NUM_THREADS":"2"});
    let changed: EnvironmentCapture = serde_json::from_value(changed).unwrap();
    assert_ne!(changed.digest(), first.digest());
}

#[test]
fn collected_host_and_clock_are_valid_in_complete_and_failed_reports() {
    let capture = EnvironmentCapture::collect(&BTreeMap::new());
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../schemas/benchmark-report-v2.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for source in [
        include_str!("../../../../examples/reports/successful.json"),
        include_str!("../../../../examples/reports/failed.json"),
    ] {
        let mut value: serde_json::Value = serde_json::from_str(source).unwrap();
        value["schema_version"] = json!("2.0.0");
        value["environment"]["host"] = serde_json::to_value(&capture.host).unwrap();
        value["environment"]["clock"] = serde_json::to_value(&capture.clock).unwrap();
        assert!(validator.is_valid(&value));
        let report: crate::BenchmarkReport = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(report).unwrap(), value);
        value["environment"]["host"]["cpu"]["physical_cores"] =
            json!({"availability":"unavailable"});
        assert!(!validator.is_valid(&value));
        assert!(serde_json::from_value::<crate::BenchmarkReport>(value).is_err());
    }
}
