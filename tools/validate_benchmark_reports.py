import argparse
import json
import math
import statistics
from collections import Counter
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SCHEMA = ROOT / "schemas" / "benchmark-report-v1.schema.json"
PROOF_PHASES = {"proving", "compression", "verification", "end_to_end"}
PROOF_COMPONENTS = {"proving", "compression", "verification"}
STATUSES = ("success", "unsupported", "failed", "timed_out", "invalid")


def _pointer(parts: Any) -> str:
    return "/" + "/".join(str(part) for part in parts)


def _phase_key(phase: dict[str, Any] | None) -> str:
    return json.dumps(phase, sort_keys=True, separators=(",", ":"))


def _is_proof_phase(phase: dict[str, Any]) -> bool:
    if phase["kind"] == "standard":
        return phase["name"] in PROOF_PHASES
    return bool(PROOF_COMPONENTS.intersection(phase["components"]))


def _is_execution_only_phase(phase: dict[str, Any]) -> bool:
    if phase["kind"] == "standard":
        return phase["name"] == "execution"
    components = set(phase["components"])
    return "execution" in components and not PROOF_COMPONENTS.intersection(components)


def _sample_value(measurement: dict[str, Any], sample: dict[str, Any]) -> int:
    if measurement["metric"] == "duration":
        return sample["timing"]["duration_ns"]
    return sample["value"]


def _percentile(values: list[int], percentile: float, method: str) -> float:
    ordered = sorted(values)
    if method == "nearest_rank":
        rank = max(1, math.ceil(percentile * len(ordered)))
        return ordered[min(rank - 1, len(ordered) - 1)]

    position = percentile * (len(ordered) - 1)
    lower = math.floor(position)
    upper = math.ceil(position)
    if method == "lower":
        return ordered[lower]
    if method == "higher":
        return ordered[upper]
    if method == "midpoint":
        return (ordered[lower] + ordered[upper]) / 2
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def _schema_issues(report: dict[str, Any], schema: dict[str, Any]) -> list[str]:
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    errors = sorted(validator.iter_errors(report), key=lambda error: list(error.path))
    return [f"schema {_pointer(error.path)}: {error.message}" for error in errors]


def _semantic_issues(report: dict[str, Any]) -> list[str]:
    issues: list[str] = []
    if (
        report["engine"]["configured_security_bits"]
        < report["benchmark"]["security_target_bits"]
    ):
        issues.append(
            "semantic /engine/configured_security_bits: configured security is below benchmark target"
        )

    percentile_method = report["run"]["policy"]["percentile_method"]
    schedule: dict[int, dict[str, Any]] = {}
    for entry in report["run"]["planned_order"]:
        position = entry["position"]
        if position in schedule:
            issues.append(f"semantic /run/planned_order: duplicate position {position}")
        schedule[position] = entry

    measurement_keys: set[tuple[str, str]] = set()
    sample_ids: set[str] = set()
    attempts: dict[str, dict[str, Any]] = {}
    proof_required: set[tuple[str, str]] = set()
    proof_sizes: Counter[tuple[str, str]] = Counter()
    proof_size_phases: dict[str, set[str]] = {}

    for measurement_index, measurement in enumerate(report["measurements"]):
        phase = measurement.get("phase")
        phase_key = _phase_key(phase)
        measurement_key = (measurement["metric"], phase_key)
        measurement_path = f"/measurements/{measurement_index}"
        if measurement_key in measurement_keys:
            issues.append(f"semantic {measurement_path}: duplicate metric and phase")
        measurement_keys.add(measurement_key)

        if measurement["availability"] == "unavailable":
            continue

        samples = measurement["samples"]
        samples_by_attempt: dict[str, dict[str, Any]] = {}
        for sample_index, sample in enumerate(samples):
            sample_path = f"{measurement_path}/samples/{sample_index}"
            sample_id = sample["id"]
            attempt_id = sample["attempt_id"]

            if sample_id in sample_ids:
                issues.append(f"semantic {sample_path}/id: duplicate sample ID")
            sample_ids.add(sample_id)

            if attempt_id in samples_by_attempt:
                issues.append(
                    f"semantic {sample_path}/attempt_id: duplicate attempt in measurement"
                )
            samples_by_attempt[attempt_id] = sample

            identity = {
                key: sample.get(key)
                for key in (
                    "attempt_index",
                    "schedule_position",
                    "warmup",
                    "started_at",
                    "finished_at",
                    "status",
                    "replacement_attempt_id",
                    "error_code",
                    "reason",
                )
            }
            previous = attempts.get(attempt_id)
            if previous is None:
                attempts[attempt_id] = identity
            elif previous != identity:
                issues.append(
                    f"semantic {sample_path}: attempt metadata differs across metrics"
                )

            planned = schedule.get(sample["schedule_position"])
            if planned is None:
                issues.append(
                    f"semantic {sample_path}/schedule_position: unknown schedule entry"
                )
            else:
                if planned["engine_id"] != report["engine"]["id"]:
                    issues.append(
                        f"semantic {sample_path}: engine does not match schedule"
                    )
                if phase is not None and planned["phase"] != phase:
                    issues.append(
                        f"semantic {sample_path}: phase does not match schedule"
                    )
                if planned["attempt_index"] != sample["attempt_index"]:
                    issues.append(
                        f"semantic {sample_path}: attempt index does not match schedule"
                    )
                if planned["warmup"] != sample["warmup"]:
                    issues.append(
                        f"semantic {sample_path}: warm-up flag does not match schedule"
                    )

            timing = sample.get("timing") or sample.get("diagnostic_timing")
            if timing is not None:
                if timing["stop_ns"] < timing["start_ns"]:
                    issues.append(f"semantic {sample_path}: stop_ns precedes start_ns")
                expected_duration = timing["stop_ns"] - timing["start_ns"]
                if timing["duration_ns"] != expected_duration:
                    issues.append(
                        f"semantic {sample_path}: duration_ns does not equal stop_ns - start_ns"
                    )

            if measurement["metric"] == "duration" and sample["status"] == "success":
                correctness = sample.get("correctness")
                expected_output = report["benchmark"]["expected_output_digest"]
                proof_phase = _is_proof_phase(phase)
                execution_phase = _is_execution_only_phase(phase)
                if proof_phase or execution_phase:
                    if correctness is None:
                        issues.append(
                            f"semantic {sample_path}: successful phase lacks correctness"
                        )
                    elif correctness.get("output_digest") != expected_output:
                        issues.append(
                            f"semantic {sample_path}/correctness/output_digest: unexpected output"
                        )

                if proof_phase:
                    proof_required.add((phase_key, attempt_id))
                    if correctness is not None:
                        if correctness.get("verification_verdict") != "accepted":
                            issues.append(
                                f"semantic {sample_path}/correctness: proof was not accepted"
                            )
                        commitment_policy = report["benchmark"]["commitment_policy"]
                        for side in ("input", "output"):
                            field = f"{side}_commitment_digest"
                            required = commitment_policy[side] != "none"
                            if required and field not in correctness:
                                issues.append(
                                    f"semantic {sample_path}/correctness: missing {field}"
                                )
                            if not required and field in correctness:
                                issues.append(
                                    f"semantic {sample_path}/correctness: inapplicable {field}"
                                )
                elif execution_phase:
                    if correctness is not None and set(correctness) != {
                        "output_digest"
                    }:
                        issues.append(
                            f"semantic {sample_path}/correctness: execution has non-applicable proof fields"
                        )
                elif correctness is not None:
                    issues.append(
                        f"semantic {sample_path}/correctness: phase has no correctness payload"
                    )

            if (
                measurement["metric"] == "duration"
                and sample["status"] == "failed"
                and sample.get("correctness") is not None
            ):
                correctness = sample["correctness"]
                if _is_proof_phase(phase):
                    if correctness.get("verification_verdict") != "rejected":
                        issues.append(
                            f"semantic {sample_path}/correctness: failed proof verdict is not rejected"
                        )
                elif _is_execution_only_phase(phase):
                    if set(correctness) != {"output_digest"}:
                        issues.append(
                            f"semantic {sample_path}/correctness: failed execution has non-applicable proof fields"
                        )
                else:
                    issues.append(
                        f"semantic {sample_path}/correctness: phase has no correctness payload"
                    )

            if measurement["metric"] == "proof_size" and sample["status"] == "success":
                proof_sizes[(phase_key, attempt_id)] += 1
                proof_size_phases.setdefault(attempt_id, set()).add(phase_key)

        summary = measurement["statistics"]
        counts = Counter(sample["status"] for sample in samples)
        for status in STATUSES:
            if summary["status_counts"][status] != counts[status]:
                issues.append(
                    f"semantic {measurement_path}/statistics/status_counts/{status}: count mismatch"
                )

        measured = [sample for sample in samples if not sample["warmup"]]
        if measurement["metric"] == "duration":
            policy = measurement["policy"]
            warmup_count = sum(sample["warmup"] for sample in samples)
            planned_measured_count = sum(
                not sample["warmup"] and sample.get("replacement_attempt_id") is None
                for sample in samples
            )
            if warmup_count != policy["warmup_count"]:
                issues.append(
                    f"semantic {measurement_path}/policy/warmup_count: count mismatch"
                )
            if planned_measured_count != policy["measured_trial_count"]:
                issues.append(
                    f"semantic {measurement_path}/policy/measured_trial_count: count mismatch"
                )

        denominator = len(measured)
        measured_counts = Counter(sample["status"] for sample in measured)
        expected_failure_rate = (
            measured_counts["failed"] / denominator if denominator else 0
        )
        expected_timeout_rate = (
            measured_counts["timed_out"] / denominator if denominator else 0
        )
        if not math.isclose(summary["rates"]["failure"], expected_failure_rate):
            issues.append(
                f"semantic {measurement_path}/statistics/rates/failure: rate mismatch"
            )
        if not math.isclose(summary["rates"]["timeout"], expected_timeout_rate):
            issues.append(
                f"semantic {measurement_path}/statistics/rates/timeout: rate mismatch"
            )

        if summary["status"] != "available":
            continue

        included_ids = summary["included_attempt_ids"]
        eligible_ids = {
            sample["attempt_id"]
            for sample in samples
            if sample["status"] == "success" and not sample["warmup"]
        }
        if set(included_ids) != eligible_ids:
            issues.append(
                f"semantic {measurement_path}/statistics/included_attempt_ids: eligible attempt set mismatch"
            )
        included = [
            samples_by_attempt[attempt_id]
            for attempt_id in included_ids
            if attempt_id in samples_by_attempt
        ]
        if len(included) != len(included_ids):
            issues.append(
                f"semantic {measurement_path}/statistics/included_attempt_ids: unknown attempt"
            )
        if summary["sample_count"] != len(included):
            issues.append(
                f"semantic {measurement_path}/statistics/sample_count: count mismatch"
            )
        if any(
            sample["status"] != "success" or sample["warmup"] for sample in included
        ):
            issues.append(
                f"semantic {measurement_path}/statistics/included_attempt_ids: ineligible attempt"
            )

        warmup_ids = {sample["attempt_id"] for sample in samples if sample["warmup"]}
        if set(summary["excluded_warmup_attempt_ids"]) != warmup_ids:
            issues.append(
                f"semantic {measurement_path}/statistics/excluded_warmup_attempt_ids: warm-up set mismatch"
            )
        if not set(summary["flagged_outlier_attempt_ids"]).issubset(set(included_ids)):
            issues.append(
                f"semantic {measurement_path}/statistics/flagged_outlier_attempt_ids: outlier not included"
            )

        if not included:
            continue
        values = [_sample_value(measurement, sample) for sample in included]
        expected_summaries = {
            "minimum": min(values),
            "maximum": max(values),
            "median": statistics.median(values),
            "mean": statistics.mean(values),
            "standard_deviation": statistics.stdev(values) if len(values) > 1 else 0,
        }
        for field, expected in expected_summaries.items():
            if field in summary and not math.isclose(summary[field], expected):
                issues.append(
                    f"semantic {measurement_path}/statistics/{field}: summary mismatch"
                )
        for name, value in summary["percentiles"].items():
            percentile = float(name[1:]) / 100
            expected = _percentile(values, percentile, percentile_method)
            if not math.isclose(value, expected):
                issues.append(
                    f"semantic {measurement_path}/statistics/percentiles/{name}: percentile mismatch"
                )

    for attempt_id, identity in attempts.items():
        replacement_id = identity.get("replacement_attempt_id")
        if replacement_id is None:
            continue
        target = attempts.get(replacement_id)
        if target is None:
            issues.append(
                f"semantic attempt {attempt_id}: replacement target does not exist"
            )
        elif target["status"] != "invalid":
            issues.append(
                f"semantic attempt {attempt_id}: replacement target is not invalid"
            )

    for phase_key, attempt_id in proof_required:
        count = proof_sizes[(phase_key, attempt_id)]
        if count != 1:
            observed_phases = sorted(proof_size_phases.get(attempt_id, set()))
            observed = ", ".join(observed_phases) if observed_phases else "none"
            issues.append(
                f"semantic attempt {attempt_id}: expected one proof_size observation "
                f"for phase {phase_key}, found {count}; observed phases: {observed}"
            )

    for collection_name in ("artifacts", "warnings"):
        for index, item in enumerate(report[collection_name]):
            attempt_id = item.get("attempt_id")
            if attempt_id is not None and attempt_id not in attempts:
                issues.append(
                    f"semantic /{collection_name}/{index}/attempt_id: unknown attempt"
                )

    outcome = report["status"]["outcome"]
    statuses = Counter(identity["status"] for identity in attempts.values())
    unavailable = any(
        measurement["availability"] == "unavailable"
        for measurement in report["measurements"]
    )
    if outcome == "successful" and (
        unavailable
        or statuses["unsupported"]
        or statuses["failed"]
        or statuses["timed_out"]
        or statuses["invalid"]
    ):
        issues.append(
            "semantic /status/outcome: successful status contradicts report data"
        )
    if outcome == "failed" and not statuses["failed"]:
        issues.append("semantic /status/outcome: failed report has no failed attempt")
    if outcome == "timed_out" and not statuses["timed_out"]:
        issues.append(
            "semantic /status/outcome: timed-out report has no timed-out attempt"
        )
    if outcome == "partially_supported" and not unavailable:
        issues.append(
            "semantic /status/outcome: partially supported report has no unavailable measurement"
        )

    return issues


def validate_report(report: dict[str, Any], schema: dict[str, Any]) -> list[str]:
    schema_issues = _schema_issues(report, schema)
    if schema_issues:
        return schema_issues
    return _semantic_issues(report)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate zkperf BenchmarkReport v1 documents."
    )
    parser.add_argument("reports", nargs="+", type=Path)
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    args = parser.parse_args()

    schema = json.loads(args.schema.read_text())
    Draft202012Validator.check_schema(schema)
    failed = False
    for path in args.reports:
        report = json.loads(path.read_text())
        issues = validate_report(report, schema)
        if issues:
            failed = True
            print(f"{path}: INVALID")
            for issue in issues:
                print(f"  {issue}")
        else:
            print(f"{path}: valid")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
