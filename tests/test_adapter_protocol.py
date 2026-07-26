import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker

from tools.validate_adapter_protocol import (
    select_protocol_version,
    validate_catalog,
    validate_exchange,
)

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_PATH = ROOT / "schemas" / "adapter-protocol-v1.schema.json"
REPORT_SCHEMA_PATH = ROOT / "schemas" / "benchmark-report-v1.schema.json"
EXAMPLES_PATH = ROOT / "examples" / "protocol-v1.json"

OPERATIONS = {
    "capabilities",
    "metadata",
    "prepare",
    "execute",
    "prove",
    "verify",
}

class AdapterProtocolTests(unittest.TestCase):
    schema: dict[str, Any]
    report_schema: dict[str, Any]
    examples: dict[str, Any]
    validator: Draft202012Validator

    @classmethod
    def setUpClass(cls) -> None:
        cls.schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
        cls.report_schema = json.loads(
            REPORT_SCHEMA_PATH.read_text(encoding="utf-8")
        )
        cls.examples = json.loads(EXAMPLES_PATH.read_text(encoding="utf-8"))
        cls.validator = Draft202012Validator(
            cls.schema,
            format_checker=FormatChecker(),
        )

    def assertValid(self, document: dict[str, Any]) -> None:
        errors = sorted(
            self.validator.iter_errors(document),
            key=lambda error: list(error.absolute_path),
        )
        messages = [
            f"{'/'.join(map(str, error.absolute_path))}: {error.message}"
            for error in errors
        ]
        self.assertEqual([], messages)

    def assertInvalid(self, document: dict[str, Any]) -> None:
        self.assertTrue(list(self.validator.iter_errors(document)))

    def exchange(self, name: str) -> dict[str, Any]:
        return next(
            exchange
            for exchange in self.examples["exchanges"]
            if exchange["name"] == name
        )

    def test_schema_is_valid_draft_2020_12(self) -> None:
        Draft202012Validator.check_schema(self.schema)

    def test_manifest_and_all_messages_validate(self) -> None:
        self.assertValid(self.examples["manifest"])
        self.assertValid(self.examples["cancellation"])
        for exchange in self.examples["exchanges"]:
            with self.subTest(exchange=exchange["name"], message="request"):
                self.assertValid(exchange["request"])
            with self.subTest(exchange=exchange["name"], message="response"):
                self.assertValid(exchange["response"])

    def test_catalog_semantics_validate(self) -> None:
        self.assertEqual([], validate_catalog(self.examples, self.schema))

    def test_malformed_catalogs_return_issues_without_crashing(self) -> None:
        catalogs = []

        missing_request_operation = copy.deepcopy(self.examples)
        del missing_request_operation["exchanges"][0]["request"]["operation"]
        catalogs.append(("missing request operation", missing_request_operation))

        missing_response_operation = copy.deepcopy(self.examples)
        del missing_response_operation["exchanges"][0]["response"]["operation"]
        catalogs.append(("missing response operation", missing_response_operation))

        missing_response = copy.deepcopy(self.examples)
        del missing_response["exchanges"][0]["response"]
        catalogs.append(("missing response", missing_response))

        invalid_exchange = copy.deepcopy(self.examples)
        invalid_exchange["exchanges"][0] = None
        catalogs.append(("invalid exchange", invalid_exchange))

        mismatched_operations = copy.deepcopy(self.examples)
        execute = next(
            exchange
            for exchange in mismatched_operations["exchanges"]
            if exchange["name"] == "execute"
        )
        metadata_response = copy.deepcopy(self.exchange("metadata")["response"])
        metadata_response["request_id"] = execute["request"]["request_id"]
        execute["response"] = metadata_response
        catalogs.append(("mismatched operations", mismatched_operations))

        for name, catalog in catalogs:
            with self.subTest(case=name):
                self.assertTrue(validate_catalog(catalog, self.schema))

    def test_cli_reports_malformed_catalog_without_traceback(self) -> None:
        catalog = copy.deepcopy(self.examples)
        del catalog["exchanges"][0]["response"]["operation"]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "malformed-catalog.json"
            path.write_text(json.dumps(catalog), encoding="utf-8")
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools" / "validate_adapter_protocol.py"),
                    str(path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(1, result.returncode)
        self.assertIn("INVALID", result.stdout)
        self.assertIn("schema", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_cli_reports_invalid_schema_without_traceback(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            malformed = root / "malformed-schema.json"
            malformed.write_text("{", encoding="utf-8")
            invalid = root / "invalid-schema.json"
            invalid.write_text('{"type": 7}', encoding="utf-8")
            paths = (
                root / "missing-schema.json",
                malformed,
                invalid,
            )

            for path in paths:
                with self.subTest(schema=path.name):
                    result = subprocess.run(
                        [
                            sys.executable,
                            str(ROOT / "tools" / "validate_adapter_protocol.py"),
                            "--schema",
                            str(path),
                            str(EXAMPLES_PATH),
                        ],
                        check=False,
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(1, result.returncode)
                    self.assertIn("INVALID SCHEMA", result.stdout)
                    self.assertNotIn("Traceback", result.stderr)

    def test_examples_cover_operations_stages_and_outcomes(self) -> None:
        requests = [exchange["request"] for exchange in self.examples["exchanges"]]
        responses = [
            exchange["response"] for exchange in self.examples["exchanges"]
        ]

        self.assertEqual(OPERATIONS, {request["operation"] for request in requests})
        self.assertEqual(
            {"initial", "transform"},
            {
                request["params"]["stage"]
                for request in requests
                if request["operation"] == "prove"
            },
        )
        self.assertEqual(
            {"success", "unsupported", "error"},
            {response["status"] for response in responses},
        )
        self.assertGreaterEqual(
            len(
                {
                    response["error"]["phase"]
                    for response in responses
                    if "error" in response
                }
            ),
            2,
        )

    def test_manifest_and_runtime_capabilities_agree(self) -> None:
        manifest = self.examples["manifest"]
        capabilities = self.exchange("capabilities")["response"]["result"]

        self.assertEqual(manifest["adapter_id"], capabilities["adapter"]["id"])
        self.assertIn(
            "1.0.0",
            set(manifest["protocol_versions"])
            & set(capabilities["supported_protocol_versions"]),
        )
        self.assertEqual(OPERATIONS, set(capabilities["operations"]))
        self.assertTrue(capabilities["proof_modes"])
        self.assertTrue(capabilities["proof_modes"][0]["transformations"])

    def test_every_response_is_correlated(self) -> None:
        for exchange in self.examples["exchanges"]:
            with self.subTest(exchange=exchange["name"]):
                self.assertEqual(
                    [],
                    validate_exchange(
                        exchange["request"],
                        exchange["response"],
                        self.schema,
                    ),
                )

    def test_artifact_paths_must_follow_workspace_ownership(self) -> None:
        request_violation = copy.deepcopy(self.exchange("execute"))
        request_violation["request"]["params"]["canonical_input"][
            "path"
        ] = "outputs/canonical-input.bin"

        response_violation = copy.deepcopy(self.exchange("execute"))
        response_violation["response"]["artifacts"][0][
            "path"
        ] = "inputs/canonical-output.bin"

        for owner, exchange, expected in (
            ("host", request_violation, "outside inputs_dir"),
            ("adapter", response_violation, "outside outputs_dir"),
        ):
            with self.subTest(owner=owner):
                self.assertValid(exchange["request"])
                self.assertValid(exchange["response"])
                issues = validate_exchange(
                    exchange["request"],
                    exchange["response"],
                    self.schema,
                )
                self.assertTrue(any(expected in issue for issue in issues))

    def test_workspace_roots_must_be_disjoint(self) -> None:
        cases = (
            (
                "equal",
                {
                    "inputs_dir": "shared",
                    "outputs_dir": "shared",
                    "control_dir": "control",
                },
                "inputs_dir and outputs_dir",
            ),
            (
                "input ancestor",
                {
                    "inputs_dir": "shared",
                    "outputs_dir": "shared/outputs",
                    "control_dir": "control",
                },
                "inputs_dir and outputs_dir",
            ),
            (
                "control descendant",
                {
                    "inputs_dir": "shared",
                    "outputs_dir": "outputs",
                    "control_dir": "shared/control",
                },
                "inputs_dir and control_dir",
            ),
        )
        for name, workspace, expected in cases:
            exchange = copy.deepcopy(self.exchange("capabilities"))
            exchange["request"]["workspace"] = workspace
            with self.subTest(case=name):
                self.assertValid(exchange["request"])
                issues = validate_exchange(
                    exchange["request"],
                    exchange["response"],
                    self.schema,
                )
                self.assertTrue(
                    any(
                        expected in issue and "disjoint roots" in issue
                        for issue in issues
                    )
                )

    def test_opaque_json_is_not_treated_as_artifact_input(self) -> None:
        fake_artifact = copy.deepcopy(
            self.exchange("execute")["request"]["params"]["canonical_input"]
        )
        fake_artifact["id"] = "opaque-fake"
        fake_artifact["path"] = "opaque/fake.bin"

        configuration = copy.deepcopy(self.exchange("execute"))
        configuration["request"]["params"]["configuration"][
            "artifact-shaped-value"
        ] = fake_artifact

        statement = copy.deepcopy(self.exchange("verify"))
        statement["request"]["params"]["statement"][
            "artifact-shaped-value"
        ] = fake_artifact

        extension = copy.deepcopy(self.exchange("execute"))
        extension["request"]["params"]["canonical_input"]["extensions"] = {
            "example.artifact": fake_artifact
        }

        for location, exchange in (
            ("configuration", configuration),
            ("statement", statement),
            ("extensions", extension),
        ):
            with self.subTest(location=location):
                self.assertValid(exchange["request"])
                self.assertEqual(
                    [],
                    validate_exchange(
                        exchange["request"],
                        exchange["response"],
                        self.schema,
                    ),
                )

    def test_response_artifact_ids_are_unique_for_every_status(self) -> None:
        source_artifact = self.exchange("prove_initial")["response"]["artifacts"][0]
        for name in ("execute", "unsupported_proof_mode", "proving_failure"):
            exchange = copy.deepcopy(self.exchange(name))
            first = copy.deepcopy(source_artifact)
            second = copy.deepcopy(source_artifact)
            second["path"] = "outputs/duplicate-proof.bin"
            second["digest"]["value"] = "4" * 64
            exchange["response"]["artifacts"].extend([first, second])

            with self.subTest(exchange=name):
                self.assertValid(exchange["response"])
                issues = validate_exchange(
                    exchange["request"],
                    exchange["response"],
                    self.schema,
                )
                self.assertTrue(any("duplicate ID" in issue for issue in issues))

    def test_unknown_normative_property_is_rejected(self) -> None:
        request = copy.deepcopy(self.exchange("capabilities")["request"])
        request["debug"] = True
        self.assertInvalid(request)

    def test_success_cannot_contain_an_error(self) -> None:
        response = copy.deepcopy(self.exchange("verify")["response"])
        response["error"] = {
            "phase": "verification",
            "code": "contradictory_state",
            "message": "A successful response cannot also be an error.",
            "retryable": False,
        }
        self.assertInvalid(response)

    def test_failure_requires_a_phase(self) -> None:
        response = copy.deepcopy(self.exchange("proving_failure")["response"])
        del response["error"]["phase"]
        self.assertInvalid(response)

    def test_transform_requires_a_transformation_id(self) -> None:
        request = copy.deepcopy(self.exchange("prove_transform")["request"])
        del request["params"]["transformation_id"]
        self.assertInvalid(request)

    def test_absolute_parent_and_empty_segment_paths_are_rejected(self) -> None:
        request = copy.deepcopy(self.exchange("execute")["request"])
        for path in (
            "/tmp/input.bin",
            "C:/secret.bin",
            "c:secret.bin",
            "Z:/nested/secret.bin",
            "inputs/../secret.bin",
            "inputs//input.bin",
            "inputs/",
        ):
            with self.subTest(path=path):
                mutated = copy.deepcopy(request)
                mutated["params"]["canonical_input"]["path"] = path
                self.assertInvalid(mutated)

        workspace_request = copy.deepcopy(
            self.exchange("capabilities")["request"]
        )
        for field in ("inputs_dir", "outputs_dir", "control_dir"):
            with self.subTest(workspace=field):
                mutated = copy.deepcopy(workspace_request)
                mutated["workspace"][field] += "/"
                self.assertInvalid(mutated)

    def test_measurement_boundaries_reject_unrelated_phases(self) -> None:
        cases = (
            ("prepare", "build", "verification"),
            ("prepare", "setup", "build"),
            ("execute", None, "build"),
            ("prove", "initial", "compression"),
            ("prove", "transform", "proving"),
            ("verify", None, "build"),
        )
        for operation, stage, phase_name in cases:
            response = copy.deepcopy(self.exchange("capabilities")["response"])
            boundary = next(
                item
                for item in response["result"]["measurement_boundaries"]
                if item["operation"] == operation and item.get("stage") == stage
            )
            boundary["phase"] = {"kind": "standard", "name": phase_name}
            with self.subTest(
                operation=operation,
                stage=stage,
                phase=phase_name,
            ):
                self.assertInvalid(response)

    def test_supported_combined_boundaries_validate(self) -> None:
        response = copy.deepcopy(self.exchange("capabilities")["response"])
        build = response["result"]["measurement_boundaries"][0]
        build["phase"] = {
            "kind": "combined",
            "components": ["build", "setup"],
        }
        proving = next(
            item
            for item in response["result"]["measurement_boundaries"]
            if item["operation"] == "prove" and item["stage"] == "initial"
        )
        proving["phase"] = {
            "kind": "combined",
            "components": ["execution", "proving"],
        }
        self.assertValid(response)

    def test_prove_result_must_match_request(self) -> None:
        cases = []

        stage = copy.deepcopy(self.exchange("prove_initial"))
        stage["response"]["result"]["stage"] = "transform"
        stage["response"]["result"]["transformation_id"] = "mock-compress"
        cases.append(("stage", stage))

        mode = copy.deepcopy(self.exchange("prove_initial"))
        mode["response"]["result"]["proof_mode_id"] = "different-mode"
        cases.append(("proof_mode_id", mode))

        transformation = copy.deepcopy(self.exchange("prove_transform"))
        transformation["response"]["result"]["transformation_id"] = "different-transform"
        cases.append(("transformation_id", transformation))

        for field, exchange in cases:
            with self.subTest(field=field):
                self.assertValid(exchange["response"])
                issues = validate_exchange(
                    exchange["request"],
                    exchange["response"],
                    self.schema,
                )
                self.assertTrue(any(f"/{field}:" in issue for issue in issues))

    def test_verify_result_must_match_expected_digests(self) -> None:
        cases = []

        output = copy.deepcopy(self.exchange("verify"))
        output["response"]["result"]["output_digest"]["value"] = "0" * 64
        cases.append(("output_digest", output))

        commitments = copy.deepcopy(self.exchange("verify"))
        commitments["response"]["result"]["commitment_digests"]["input"][
            "value"
        ] = "0" * 64
        cases.append(("commitment_digests", commitments))

        benchmark = copy.deepcopy(self.exchange("verify"))
        benchmark["request"]["params"]["expected_output_digest"]["value"] = "0" * 64
        benchmark["response"]["result"]["output_digest"]["value"] = "0" * 64
        cases.append(("expected_output_digest", benchmark))

        for field, exchange in cases:
            with self.subTest(field=field):
                self.assertValid(exchange["request"])
                self.assertValid(exchange["response"])
                issues = validate_exchange(
                    exchange["request"],
                    exchange["response"],
                    self.schema,
                )
                self.assertTrue(any(f"/{field}:" in issue for issue in issues))

    def test_proving_support_requires_at_least_one_proof_mode(self) -> None:
        response = copy.deepcopy(self.exchange("capabilities")["response"])
        response["result"]["proof_modes"] = []
        self.assertInvalid(response)

    def test_duplicate_proof_mode_and_transformation_ids_are_rejected(self) -> None:
        mode_catalog = copy.deepcopy(self.examples)
        capabilities = next(
            exchange
            for exchange in mode_catalog["exchanges"]
            if exchange["name"] == "capabilities"
        )["response"]["result"]
        duplicate_mode = copy.deepcopy(capabilities["proof_modes"][0])
        duplicate_mode["display_name"] = "Duplicate ID with different metadata"
        capabilities["proof_modes"].append(duplicate_mode)

        self.assertValid(
            next(
                exchange
                for exchange in mode_catalog["exchanges"]
                if exchange["name"] == "capabilities"
            )["response"]
        )
        self.assertTrue(
            any(
                "duplicate ID" in issue
                for issue in validate_catalog(mode_catalog, self.schema)
            )
        )

        transformation_catalog = copy.deepcopy(self.examples)
        capabilities = next(
            exchange
            for exchange in transformation_catalog["exchanges"]
            if exchange["name"] == "capabilities"
        )["response"]["result"]
        duplicate_transformation = copy.deepcopy(
            capabilities["proof_modes"][0]["transformations"][0]
        )
        duplicate_transformation["output_format"] = "application/x-duplicate"
        capabilities["proof_modes"][0]["transformations"].append(
            duplicate_transformation
        )

        self.assertValid(
            next(
                exchange
                for exchange in transformation_catalog["exchanges"]
                if exchange["name"] == "capabilities"
            )["response"]
        )
        self.assertTrue(
            any(
                "duplicate ID" in issue
                for issue in validate_catalog(transformation_catalog, self.schema)
            )
        )

    def test_selected_version_must_be_advertised_by_both_peers(self) -> None:
        catalog = copy.deepcopy(self.examples)
        catalog["manifest"]["protocol_versions"] = ["1.1.0"]
        capabilities = next(
            exchange
            for exchange in catalog["exchanges"]
            if exchange["name"] == "capabilities"
        )
        capabilities["response"]["result"]["supported_protocol_versions"] = [
            "1.1.0"
        ]

        issues = validate_catalog(catalog, self.schema)
        self.assertTrue(
            any("selected version is not advertised" in issue for issue in issues)
        )

    def test_selected_version_must_be_highest_exact_semver(self) -> None:
        self.assertEqual(
            "1.1.0",
            select_protocol_version(
                ["1.0.0", "1.1.0"],
                ["1.1.0", "1.0.0"],
            ),
        )
        self.assertEqual(
            "2.0.0-alpha.10",
            select_protocol_version(
                ["2.0.0-alpha.2", "2.0.0-alpha.10"],
                ["2.0.0-alpha.2", "2.0.0-alpha.10"],
            ),
        )
        self.assertEqual(
            "2.0.0",
            select_protocol_version(
                ["2.0.0-alpha.2", "2.0.0-alpha.10", "2.0.0"],
                ["2.0.0-alpha.2", "2.0.0-alpha.10", "2.0.0"],
            ),
        )

        catalog = copy.deepcopy(self.examples)
        catalog["manifest"]["protocol_versions"] = ["1.0.0", "1.1.0"]
        capabilities = catalog["exchanges"][0]
        capabilities["request"]["params"]["host"][
            "supported_protocol_versions"
        ] = ["1.0.0", "1.1.0"]
        capabilities["response"]["result"]["supported_protocol_versions"] = [
            "1.0.0",
            "1.1.0",
        ]

        issues = validate_catalog(catalog, self.schema)
        self.assertTrue(
            any("expected highest exact 1.1.0" in issue for issue in issues)
        )

    def test_capability_declarations_constrain_all_outcomes(self) -> None:
        execute_disabled = copy.deepcopy(self.examples)
        capabilities = execute_disabled["exchanges"][0]["response"]["result"]
        capabilities["operations"]["execute"] = {
            "supported": False,
            "reason": {
                "code": "disabled",
                "message": "Execution is disabled.",
            },
        }

        prove_disabled = copy.deepcopy(self.examples)
        capabilities = prove_disabled["exchanges"][0]["response"]["result"]
        capabilities["operations"]["prove"] = {
            "supported": False,
            "reason": {
                "code": "disabled",
                "message": "Proving is disabled.",
            },
        }

        transform_absent = copy.deepcopy(self.examples)
        capabilities = transform_absent["exchanges"][0]["response"]["result"]
        capabilities["operations"]["prove"]["stages"] = ["initial"]

        cases = (
            ("execute disabled", execute_disabled, "unadvertised execute operation"),
            ("prove disabled", prove_disabled, "unadvertised prove operation"),
            (
                "transform absent",
                transform_absent,
                "unadvertised prove/transform stage",
            ),
        )
        for name, catalog, expected in cases:
            with self.subTest(case=name):
                self.assertTrue(
                    any(
                        expected in issue
                        for issue in validate_catalog(catalog, self.schema)
                    )
                )

        error_for_unknown_mode = copy.deepcopy(self.examples)
        proving_failure = next(
            exchange
            for exchange in error_for_unknown_mode["exchanges"]
            if exchange["name"] == "proving_failure"
        )
        proving_failure["request"]["params"]["proof_mode_id"] = "unknown-mode"
        self.assertTrue(
            any(
                "unadvertised proof mode unknown-mode must return unsupported"
                in issue
                for issue in validate_catalog(error_for_unknown_mode, self.schema)
            )
        )

    def test_proof_artifact_formats_match_capabilities(self) -> None:
        for name, expected_format in (
            ("prove_initial", "application/vnd.zkperf.mock-proof"),
            (
                "prove_transform",
                "application/vnd.zkperf.mock-proof+compressed",
            ),
        ):
            catalog = copy.deepcopy(self.examples)
            exchange = next(
                item for item in catalog["exchanges"] if item["name"] == name
            )
            proof_id = exchange["response"]["result"]["proof_artifact_id"]
            proof = next(
                artifact
                for artifact in exchange["response"]["artifacts"]
                if artifact["id"] == proof_id
            )
            proof["media_type"] = "application/x-wrong-proof-format"

            with self.subTest(exchange=name):
                issues = validate_catalog(catalog, self.schema)
                self.assertTrue(
                    any(
                        "proof artifact must use" in issue
                        and expected_format in issue
                        for issue in issues
                    )
                )

        for field in ("proof_format", "output_format"):
            response = copy.deepcopy(self.exchange("capabilities")["response"])
            mode = response["result"]["proof_modes"][0]
            if field == "proof_format":
                mode[field] = "not a media type"
            else:
                mode["transformations"][0][field] = "not a media type"
            with self.subTest(capability_field=field):
                self.assertInvalid(response)

    def test_advertised_artifact_limits_constrain_responses(self) -> None:
        cases = (
            ("max_artifact_count", 1, "count exceeds advertised limit"),
            ("max_artifact_bytes", 1, "byte_length: exceeds advertised limit"),
            (
                "max_total_artifact_bytes",
                1,
                "total byte length exceeds advertised limit",
            ),
        )
        for field, value, expected in cases:
            catalog = copy.deepcopy(self.examples)
            limits = catalog["exchanges"][0]["response"]["result"]["limits"]
            limits[field] = value
            with self.subTest(limit=field):
                self.assertTrue(
                    any(
                        expected in issue
                        for issue in validate_catalog(catalog, self.schema)
                    )
                )

    def test_artifact_limits_activate_after_capability_exchange(self) -> None:
        catalog = copy.deepcopy(self.examples)
        capability_exchange = catalog["exchanges"][0]
        catalog["exchanges"] = [capability_exchange]
        catalog["cancellation"]["request_id"] = capability_exchange["request"][
            "request_id"
        ]

        artifact = copy.deepcopy(
            self.exchange("prepare_build")["response"]["artifacts"][0]
        )
        artifact["id"] = "capability-artifact"
        artifact["path"] = "outputs/capability-artifact.bin"
        capability_exchange["response"]["artifacts"].append(artifact)
        limits = capability_exchange["response"]["result"]["limits"]
        limits["max_artifact_count"] = 0
        limits["max_artifact_bytes"] = 0
        limits["max_total_artifact_bytes"] = 0

        self.assertEqual([], validate_catalog(catalog, self.schema))

    def test_metadata_adapter_identity_matches_capabilities(self) -> None:
        replacements = {
            "id": "different-adapter",
            "name": "Different adapter",
            "version": "9.9.9",
            "revision": "different-revision",
        }
        for field, value in replacements.items():
            catalog = copy.deepcopy(self.examples)
            metadata = next(
                exchange
                for exchange in catalog["exchanges"]
                if exchange["name"] == "metadata"
            )
            metadata["response"]["result"]["adapter"][field] = value
            with self.subTest(field=field):
                self.assertTrue(
                    any(
                        "/response/result/adapter: "
                        "does not match runtime capabilities" in issue
                        for issue in validate_catalog(catalog, self.schema)
                    )
                )

    def test_canonical_artifact_fields_require_semantic_kinds(self) -> None:
        wrong_execute_input = copy.deepcopy(self.exchange("execute")["request"])
        wrong_execute_input["params"]["canonical_input"]["kind"] = "proof"
        self.assertInvalid(wrong_execute_input)

        wrong_verify_proof = copy.deepcopy(self.exchange("verify")["request"])
        wrong_verify_proof["params"]["proof"]["kind"] = "canonical_input"
        self.assertInvalid(wrong_verify_proof)

        wrong_initial_input = copy.deepcopy(
            self.exchange("prove_initial")["request"]
        )
        wrong_initial_input["params"]["input_artifacts"][0]["kind"] = "proof"
        self.assertInvalid(wrong_initial_input)

        wrong_transform_input = copy.deepcopy(
            self.exchange("prove_transform")["request"]
        )
        wrong_transform_input["params"]["input_artifacts"][0][
            "kind"
        ] = "execution_trace"
        self.assertInvalid(wrong_transform_input)

        output_cases = []
        wrong_execute_output = copy.deepcopy(self.exchange("execute"))
        wrong_execute_output["response"]["artifacts"][0]["kind"] = "proof"
        output_cases.append(
            (
                wrong_execute_output,
                "canonical_output_artifact_id",
                "canonical_output",
            )
        )

        wrong_execution_trace = copy.deepcopy(self.exchange("execute"))
        wrong_execution_trace["response"]["artifacts"][1]["kind"] = "proof"
        output_cases.append(
            (
                wrong_execution_trace,
                "execution_artifact_id",
                "execution_trace",
            )
        )

        wrong_proof = copy.deepcopy(self.exchange("prove_initial"))
        wrong_proof["response"]["artifacts"][0]["kind"] = "profile"
        output_cases.append((wrong_proof, "proof_artifact_id", "proof"))

        wrong_public_values = copy.deepcopy(self.exchange("prove_initial"))
        wrong_public_values["response"]["artifacts"][1]["kind"] = "profile"
        output_cases.append(
            (
                wrong_public_values,
                "public_values_artifact_id",
                "public_values",
            )
        )

        for exchange, field, expected_kind in output_cases:
            with self.subTest(field=field):
                self.assertValid(exchange["response"])
                issues = validate_exchange(
                    exchange["request"],
                    exchange["response"],
                    self.schema,
                )
                self.assertTrue(
                    any(
                        field in issue and expected_kind in issue
                        for issue in issues
                    )
                )

    def test_failure_phase_must_match_operation_stage(self) -> None:
        cases = (
            ("capabilities", None, "capabilities", "verification"),
            ("metadata", None, "metadata", "execution"),
            ("prepare_build", "environment", "preparation", "setup"),
            ("prepare_build", "build", "build", "setup"),
            ("prepare_build", "setup", "setup", "build"),
            ("execute", None, "execution", "proving"),
            ("prove_initial", None, "proving", "compression"),
            ("prove_transform", None, "compression", "verification"),
            ("verify", None, "verification", "proving"),
        )
        for name, stage, expected_phase, wrong_phase in cases:
            exchange = copy.deepcopy(self.exchange(name))
            if stage is not None:
                exchange["request"]["params"]["stage"] = stage
            exchange["response"] = {
                "protocol": exchange["request"]["protocol"],
                "protocol_version": exchange["request"]["protocol_version"],
                "request_id": exchange["request"]["request_id"],
                "operation": exchange["request"]["operation"],
                "status": "error",
                "error": {
                    "phase": wrong_phase,
                    "code": "operation_failed",
                    "message": "The requested operation failed.",
                    "retryable": False,
                },
                "artifacts": [],
            }
            with self.subTest(operation=name, stage=stage):
                self.assertValid(exchange["request"])
                self.assertValid(exchange["response"])
                self.assertTrue(
                    any(
                        f"requires {expected_phase} or a cross-cutting phase"
                        in issue
                        for issue in validate_exchange(
                            exchange["request"],
                            exchange["response"],
                            self.schema,
                        )
                    )
                )

        exchange = copy.deepcopy(self.exchange("prove_transform"))
        response = copy.deepcopy(exchange["response"])
        response["status"] = "error"
        del response["result"]
        response["artifacts"] = []
        response["error"] = {
            "phase": "protocol",
            "code": "transform_failed",
            "message": "The proof transformation failed.",
            "retryable": False,
        }
        for phase in ("protocol", "artifact", "cleanup"):
            with self.subTest(cross_cutting_phase=phase):
                response["error"]["phase"] = phase
                self.assertEqual(
                    [],
                    validate_exchange(exchange["request"], response, self.schema),
                )

    def test_protocol_measurement_phases_align_with_report_v1(self) -> None:
        report_phases = set(
            self.report_schema["$defs"]["standardPhase"]["properties"]["name"][
                "enum"
            ]
        )
        boundaries = self.exchange("capabilities")["response"]["result"][
            "measurement_boundaries"
        ]
        adapter_phases: set[str] = set()
        for boundary in boundaries:
            phase = boundary["phase"]
            if phase["kind"] == "standard":
                adapter_phases.add(phase["name"])
            else:
                adapter_phases.update(phase["components"])

        self.assertEqual(
            {
                "build",
                "setup",
                "execution",
                "proving",
                "compression",
                "verification",
            },
            adapter_phases,
        )
        self.assertLessEqual(adapter_phases, report_phases)
        self.assertIn("end_to_end", report_phases)
        self.assertNotIn("end_to_end", adapter_phases)

    def test_protocol_outcomes_and_artifacts_normalize_to_report_v1(self) -> None:
        protocol_statuses = set(
            self.schema["$defs"]["responseEnvelope"]["properties"]["status"][
                "enum"
            ]
        )
        report_statuses = set(
            self.report_schema["$defs"]["sampleIdentity"]["properties"][
                "status"
            ]["enum"]
        )
        status_mapping = {
            "success": "success",
            "unsupported": "unsupported",
            "error": "failed",
        }
        self.assertEqual(protocol_statuses, set(status_mapping))
        self.assertLessEqual(set(status_mapping.values()), report_statuses)
        self.assertLessEqual({"timed_out", "invalid"}, report_statuses)

        protocol_artifact_kinds = set(
            self.schema["$defs"]["artifact"]["properties"]["kind"]["enum"]
        )
        report_artifact_kinds = set(
            self.report_schema["$defs"]["artifact"]["properties"]["kind"]["enum"]
        )
        artifact_mapping = {
            kind: "other" for kind in protocol_artifact_kinds
        }
        artifact_mapping.update(
            {
                "execution_trace": "trace",
                "proof": "proof",
                "proving_key": "proving_key",
                "verification_key": "verification_key",
                "profile": "profile",
            }
        )
        self.assertLessEqual(
            set(artifact_mapping.values()),
            report_artifact_kinds,
        )

    def test_capability_boundaries_must_be_complete_and_unique(self) -> None:
        missing = copy.deepcopy(self.examples)
        boundaries = missing["exchanges"][0]["response"]["result"][
            "measurement_boundaries"
        ]
        boundaries[:] = [
            boundary
            for boundary in boundaries
            if boundary["operation"] != "execute"
        ]
        self.assertTrue(
            any(
                "missing boundary for execute" in issue
                for issue in validate_catalog(missing, self.schema)
            )
        )

        duplicate = copy.deepcopy(self.examples)
        boundaries = duplicate["exchanges"][0]["response"]["result"][
            "measurement_boundaries"
        ]
        boundaries.append(copy.deepcopy(boundaries[0]))
        self.assertTrue(validate_catalog(duplicate, self.schema))

    def test_capabilities_must_be_first_and_request_ids_unique(self) -> None:
        late = copy.deepcopy(self.examples)
        late["exchanges"][0], late["exchanges"][1] = (
            late["exchanges"][1],
            late["exchanges"][0],
        )
        self.assertTrue(
            any(
                "first exchange must be successful capabilities" in issue
                for issue in validate_catalog(late, self.schema)
            )
        )

        duplicate = copy.deepcopy(self.examples)
        duplicate_id = duplicate["exchanges"][0]["request"]["request_id"]
        duplicate["exchanges"][1]["request"]["request_id"] = duplicate_id
        duplicate["exchanges"][1]["response"]["request_id"] = duplicate_id
        self.assertTrue(
            any(
                "duplicate request ID" in issue
                for issue in validate_catalog(duplicate, self.schema)
            )
        )

    def test_response_correlation_mismatch_is_detectable(self) -> None:
        exchange = self.exchange("execute")
        response = copy.deepcopy(exchange["response"])
        response["request_id"] = "20000000-0000-4000-8000-000000000004"

        self.assertValid(response)
        self.assertIn(
            "semantic /response/request_id: does not match request",
            validate_exchange(exchange["request"], response, self.schema),
        )


if __name__ == "__main__":
    unittest.main()
