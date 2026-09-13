use serde_json::{Value, json};
use zkperf_core::{
    BenchmarkReport, BenchmarkReportV1, BenchmarkReportV1Parts, BenchmarkReportV2, ReportError,
};

const V1: &str = include_str!("../../../schemas/benchmark-report-v1.schema.json");
const V2: &str = include_str!("../../../schemas/benchmark-report-v2.schema.json");
const EXAMPLE: &str = include_str!("../../../examples/reports/successful.json");

fn parts(value: &Value) -> BenchmarkReportV1Parts {
    BenchmarkReportV1Parts {
        report_id: serde_json::from_value(value["report_id"].clone()).unwrap(),
        contract: serde_json::from_value(value["contract"].clone()).unwrap(),
        run: serde_json::from_value(value["run"].clone()).unwrap(),
        benchmark: serde_json::from_value(value["benchmark"].clone()).unwrap(),
        engine: serde_json::from_value(value["engine"].clone()).unwrap(),
        environment: serde_json::from_value(value["environment"].clone()).unwrap(),
        measurements: serde_json::from_value(value["measurements"].clone()).unwrap(),
        artifacts: serde_json::from_value(value["artifacts"].clone()).unwrap(),
        warnings: serde_json::from_value(value["warnings"].clone()).unwrap(),
        status: serde_json::from_value(value["status"].clone()).unwrap(),
        extensions: serde_json::from_value(value["extensions"].clone()).unwrap(),
    }
}

#[test]
fn every_metadata_gap_requires_v2_in_schema_parser_and_constructor() {
    let v1 = jsonschema::validator_for(&serde_json::from_str::<Value>(V1).unwrap()).unwrap();
    let v2 = jsonschema::validator_for(&serde_json::from_str::<Value>(V2).unwrap()).unwrap();
    for field in [
        "/host/machine_id",
        "/host/architecture",
        "/host/ram_bytes",
        "/host/accelerators",
        "/host/storage",
        "/host/firmware_or_microcode",
        "/host/cpu/model",
        "/host/cpu/stepping",
        "/host/cpu/physical_cores",
        "/host/cpu/logical_cores",
        "/host/operating_system/name",
        "/host/operating_system/version",
        "/host/operating_system/kernel",
        "/clock/resolution_ns",
    ] {
        let mut value: Value = serde_json::from_str(EXAMPLE).unwrap();
        *value["environment"].pointer_mut(field).unwrap() = json!({
            "availability": "unavailable",
            "reason": {"code": "not_observed", "message": "Not collected."}
        });
        assert!(!v1.is_valid(&value), "{field}");
        assert!(
            serde_json::from_value::<BenchmarkReport>(value.clone()).is_err(),
            "{field}"
        );
        assert!(
            serde_json::from_value::<BenchmarkReportV1>(value.clone()).is_err(),
            "{field}"
        );
        assert!(
            matches!(
                BenchmarkReportV1::new(parts(&value)),
                Err(ReportError::UnavailableMetadataInV1)
            ),
            "{field}"
        );

        let constructed = BenchmarkReportV2::new(parts(&value)).unwrap();
        value["schema_version"] = json!("2.0.0");
        assert_eq!(serde_json::to_value(constructed).unwrap(), value, "{field}");
        assert!(v2.is_valid(&value), "{field}");
        let restored: BenchmarkReport = serde_json::from_value(value.clone()).unwrap();
        assert!(restored.as_v1().is_none());
        assert!(restored.as_v2().is_some());
        assert_eq!(serde_json::to_value(restored).unwrap(), value, "{field}");
        assert!(
            serde_json::from_value::<BenchmarkReportV1>(value).is_err(),
            "{field}"
        );
    }
}

#[test]
fn exact_versions_round_trip_without_silently_upgrading() {
    for source in [
        EXAMPLE,
        include_str!("../../../examples/reports/failed.json"),
        include_str!("../../../examples/reports/timed-out.json"),
        include_str!("../../../examples/reports/partially-supported.json"),
    ] {
        let mut value: Value = serde_json::from_str(source).unwrap();
        assert!(serde_json::from_value::<BenchmarkReportV2>(value.clone()).is_err());
        for version in ["1.0.0", "2.0.0"] {
            value["schema_version"] = json!(version);
            let report: BenchmarkReport = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(report).unwrap(), value);
        }
    }
}
