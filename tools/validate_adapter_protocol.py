import argparse
import json
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SCHEMA = ROOT / "schemas" / "adapter-protocol-v1.schema.json"


def _pointer(parts: Any) -> str:
    return "/" + "/".join(str(part) for part in parts)


def _schema_issues(document: Any, schema: dict[str, Any], path: str) -> list[str]:
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    errors = sorted(validator.iter_errors(document), key=lambda error: list(error.path))
    return [
        f"schema {path}{_pointer(error.path)}: {error.message}" for error in errors
    ]


def _catalog_shape_issues(catalog: Any) -> list[str]:
    if not isinstance(catalog, dict):
        return ["shape /: catalog must be an object"]

    issues: list[str] = []
    expected_types = {
        "manifest": dict,
        "cancellation": dict,
        "exchanges": list,
    }
    for field, expected_type in expected_types.items():
        value = catalog.get(field)
        if not isinstance(value, expected_type):
            issues.append(
                f"shape /{field}: must be {expected_type.__name__}"
            )

    exchanges = catalog.get("exchanges")
    if not isinstance(exchanges, list):
        return issues
    for index, exchange in enumerate(exchanges):
        path = f"/exchanges/{index}"
        if not isinstance(exchange, dict):
            issues.append(f"shape {path}: exchange must be an object")
            continue
        if not isinstance(exchange.get("name"), str) or not exchange["name"]:
            issues.append(f"shape {path}/name: must be a non-empty string")
        for field in ("request", "response"):
            if not isinstance(exchange.get(field), dict):
                issues.append(f"shape {path}/{field}: must be an object")
    return issues


def _semver_precedence_key(
    version: str,
) -> tuple[int, int, int, tuple[Any, ...]]:
    precedence = version.split("+", 1)[0]
    core, separator, prerelease = precedence.partition("-")
    major, minor, patch = (int(part) for part in core.split("."))
    if not separator:
        prerelease_key: tuple[Any, ...] = (1,)
    else:
        identifiers = tuple(
            (0, int(identifier))
            if identifier.isdigit()
            else (1, identifier)
            for identifier in prerelease.split(".")
        )
        prerelease_key = (0, identifiers)
    return major, minor, patch, prerelease_key


def select_protocol_version(
    host_versions: list[str],
    manifest_versions: list[str],
) -> str | None:
    common = set(host_versions) & set(manifest_versions)
    if not common:
        return None
    return max(
        common,
        key=lambda version: (_semver_precedence_key(version), version),
    )


def _duplicate_id_issues(items: list[dict[str, Any]], path: str) -> list[str]:
    issues: list[str] = []
    seen: set[str] = set()
    for index, item in enumerate(items):
        item_id = item["id"]
        if item_id in seen:
            issues.append(f"semantic {path}/{index}/id: duplicate ID {item_id}")
        seen.add(item_id)
    return issues


def _request_artifacts(request: dict[str, Any]) -> list[dict[str, Any]]:
    operation = request["operation"]
    params = request["params"]
    if operation == "metadata":
        return params["artifacts"]
    if operation in {"prepare", "prove"}:
        return params["input_artifacts"]
    if operation == "execute":
        return [params["canonical_input"], *params["prepared_artifacts"]]
    if operation == "verify":
        return [params["proof"]]
    return []


def _result_artifact_ids(result: dict[str, Any]) -> set[str]:
    references: set[str] = set()
    for key, value in result.items():
        if key.endswith("_artifact_id"):
            references.add(value)
        elif key.endswith("_artifact_ids"):
            references.update(value)
    return references


def _boundary_key(boundary: dict[str, Any]) -> tuple[str, str | None]:
    return boundary["operation"], boundary.get("stage")


def _expected_boundary_keys(
    operations: dict[str, dict[str, Any]],
) -> set[tuple[str, str | None]]:
    expected: set[tuple[str, str | None]] = set()
    prepare = operations["prepare"]
    if prepare["supported"]:
        expected.update(
            ("prepare", stage)
            for stage in prepare["stages"]
            if stage in {"build", "setup"}
        )
    if operations["execute"]["supported"]:
        expected.add(("execute", None))
    prove = operations["prove"]
    if prove["supported"]:
        expected.update(("prove", stage) for stage in prove["stages"])
    if operations["verify"]["supported"]:
        expected.add(("verify", None))
    return expected


def _capability_issues(result: dict[str, Any]) -> list[str]:
    operations = result["operations"]
    proof_modes = result["proof_modes"]
    issues = _duplicate_id_issues(proof_modes, "/capabilities/proof_modes")
    if (
        operations["prove"]["supported"] or operations["verify"]["supported"]
    ) and not proof_modes:
        issues.append(
            "semantic /capabilities/proof_modes: proof support requires a proof mode"
        )

    for mode_index, mode in enumerate(proof_modes):
        issues.extend(
            _duplicate_id_issues(
                mode["transformations"],
                f"/capabilities/proof_modes/{mode_index}/transformations",
            )
        )

    prove = operations["prove"]
    transformations = [
        transformation
        for mode in proof_modes
        for transformation in mode["transformations"]
    ]
    if prove["supported"]:
        transform_supported = "transform" in prove["stages"]
        if transform_supported and not transformations:
            issues.append(
                "semantic /capabilities/proof_modes: transform stage lacks a transformation"
            )
        if not transform_supported and transformations:
            issues.append(
                "semantic /capabilities/proof_modes: transformations advertised without transform stage"
            )

    expected = _expected_boundary_keys(operations)
    observed: set[tuple[str, str | None]] = set()
    for index, boundary in enumerate(result["measurement_boundaries"]):
        key = _boundary_key(boundary)
        if key in observed:
            operation, stage = key
            suffix = f"/{stage}" if stage is not None else ""
            issues.append(
                "semantic "
                f"/capabilities/measurement_boundaries/{index}: "
                f"duplicate boundary for {operation}{suffix}"
            )
        observed.add(key)

    for operation, stage in sorted(expected - observed):
        suffix = f"/{stage}" if stage is not None else ""
        issues.append(
            "semantic /capabilities/measurement_boundaries: "
            f"missing boundary for {operation}{suffix}"
        )
    for operation, stage in sorted(observed - expected):
        suffix = f"/{stage}" if stage is not None else ""
        issues.append(
            "semantic /capabilities/measurement_boundaries: "
            f"unexpected boundary for {operation}{suffix}"
        )
    return issues


def _artifact_ownership_issues(
    request: dict[str, Any],
    response: dict[str, Any],
) -> list[str]:
    issues: list[str] = []
    input_prefix = f"{request['workspace']['inputs_dir']}/"
    output_prefix = f"{request['workspace']['outputs_dir']}/"
    for artifact in _request_artifacts(request):
        if not artifact["path"].startswith(input_prefix):
            issues.append(
                "semantic /request/params: "
                f"artifact {artifact['id']} is outside inputs_dir"
            )
    for index, artifact in enumerate(response["artifacts"]):
        if not artifact["path"].startswith(output_prefix):
            issues.append(
                f"semantic /response/artifacts/{index}/path: outside outputs_dir"
            )
    return issues


def _workspace_issues(request: dict[str, Any]) -> list[str]:
    roots = request["workspace"]
    fields = ("inputs_dir", "outputs_dir", "control_dir")
    issues: list[str] = []
    for left_index, left_field in enumerate(fields):
        left = roots[left_field]
        for right_field in fields[left_index + 1 :]:
            right = roots[right_field]
            if (
                left == right
                or left.startswith(f"{right}/")
                or right.startswith(f"{left}/")
            ):
                issues.append(
                    "semantic /request/workspace: "
                    f"{left_field} and {right_field} must be disjoint roots"
                )
    return issues


def _request_artifact_kind_issues(request: dict[str, Any]) -> list[str]:
    operation = request["operation"]
    params = request["params"]
    issues: list[str] = []

    if (
        operation == "execute"
        and params["canonical_input"]["kind"] != "canonical_input"
    ):
        issues.append(
            "semantic /request/params/canonical_input/kind: "
            "must be canonical_input"
        )
    if operation == "verify" and params["proof"]["kind"] != "proof":
        issues.append("semantic /request/params/proof/kind: must be proof")
    if operation == "prove":
        expected_kind = (
            "execution_trace" if params["stage"] == "initial" else "proof"
        )
        if not any(
            artifact["kind"] == expected_kind
            for artifact in params["input_artifacts"]
        ):
            issues.append(
                "semantic /request/params/input_artifacts: "
                f"{params['stage']} proving requires {expected_kind}"
            )
    return issues


def _referenced_artifact_kind_issue(
    artifacts: dict[str, dict[str, Any]],
    artifact_id: str,
    expected_kind: str,
    path: str,
) -> str | None:
    artifact = artifacts.get(artifact_id)
    if artifact is None or artifact["kind"] == expected_kind:
        return None
    return f"semantic {path}: referenced artifact must be {expected_kind}"


def _success_result_issues(
    request: dict[str, Any],
    response: dict[str, Any],
) -> list[str]:
    operation = request["operation"]
    params = request["params"]
    result = response["result"]
    issues: list[str] = []

    artifacts = {artifact["id"]: artifact for artifact in response["artifacts"]}
    for artifact_id in sorted(_result_artifact_ids(result) - artifacts.keys()):
        issues.append(
            f"semantic /response/result: unknown artifact ID {artifact_id}"
        )

    if operation == "prepare" and result["stage"] != params["stage"]:
        issues.append(
            "semantic /response/result/stage: does not match requested prepare stage"
        )

    if operation == "execute":
        output = artifacts.get(result["canonical_output_artifact_id"])
        expected = params["benchmark"]["expected_output_digest"]
        if output is not None and output["digest"] != expected:
            issues.append(
                "semantic /response/artifacts: canonical output digest does not match benchmark"
            )
        for field, expected_kind in (
            ("canonical_output_artifact_id", "canonical_output"),
            ("execution_artifact_id", "execution_trace"),
        ):
            issue = _referenced_artifact_kind_issue(
                artifacts,
                result[field],
                expected_kind,
                f"/response/result/{field}",
            )
            if issue is not None:
                issues.append(issue)

    if operation == "prove":
        for field in ("stage", "proof_mode_id", "transformation_id"):
            if result.get(field) != params.get(field):
                issues.append(
                    f"semantic /response/result/{field}: does not match prove request"
                )
        issue = _referenced_artifact_kind_issue(
            artifacts,
            result["proof_artifact_id"],
            "proof",
            "/response/result/proof_artifact_id",
        )
        if issue is not None:
            issues.append(issue)
        public_values_id = result.get("public_values_artifact_id")
        if public_values_id is not None:
            issue = _referenced_artifact_kind_issue(
                artifacts,
                public_values_id,
                "public_values",
                "/response/result/public_values_artifact_id",
            )
            if issue is not None:
                issues.append(issue)

    if operation == "verify":
        expected_output = params["expected_output_digest"]
        benchmark_output = params["benchmark"]["expected_output_digest"]
        if expected_output != benchmark_output:
            issues.append(
                "semantic /request/params/expected_output_digest: does not match benchmark"
            )
        if result["output_digest"] != expected_output:
            issues.append(
                "semantic /response/result/output_digest: does not match verify request"
            )
        if result["commitment_digests"] != params["expected_commitment_digests"]:
            issues.append(
                "semantic /response/result/commitment_digests: do not match verify request"
            )
    return issues


def _expected_failure_phase(request: dict[str, Any]) -> str:
    operation = request["operation"]
    if operation == "capabilities":
        return "capabilities"
    if operation == "metadata":
        return "metadata"
    if operation == "prepare":
        return {
            "environment": "preparation",
            "build": "build",
            "setup": "setup",
        }[request["params"]["stage"]]
    if operation == "execute":
        return "execution"
    if operation == "prove":
        return {
            "initial": "proving",
            "transform": "compression",
        }[request["params"]["stage"]]
    return "verification"


def _failure_phase_issues(
    request: dict[str, Any],
    response: dict[str, Any],
) -> list[str]:
    if response["status"] == "success":
        return []
    actual = response["error"]["phase"]
    if actual in {"protocol", "artifact", "cleanup"}:
        return []
    expected = _expected_failure_phase(request)
    if actual == expected:
        return []
    return [
        "semantic /response/error/phase: "
        f"{request['operation']} request requires {expected} or a cross-cutting phase"
    ]


def _exchange_semantic_issues(
    request: dict[str, Any],
    response: dict[str, Any],
) -> list[str]:
    issues: list[str] = []
    correlation_issues: list[str] = []
    for field in ("protocol", "protocol_version", "request_id", "operation"):
        if request[field] != response[field]:
            correlation_issues.append(
                f"semantic /response/{field}: does not match request"
            )

    issues.extend(correlation_issues)
    issues.extend(_workspace_issues(request))
    issues.extend(_artifact_ownership_issues(request, response))
    issues.extend(_request_artifact_kind_issues(request))
    issues.extend(_duplicate_id_issues(response["artifacts"], "/response/artifacts"))
    if not correlation_issues:
        issues.extend(_failure_phase_issues(request, response))
    if response["status"] == "success" and not correlation_issues:
        issues.extend(_success_result_issues(request, response))
    return issues


def validate_exchange(
    request: Any,
    response: Any,
    schema: dict[str, Any],
) -> list[str]:
    issues = _schema_issues(request, schema, "/request")
    issues.extend(_schema_issues(response, schema, "/response"))
    if issues:
        return issues
    return _exchange_semantic_issues(request, response)


def _exchange_capability_issues(
    exchanges: list[dict[str, Any]],
    capabilities: dict[str, Any],
) -> list[str]:
    issues: list[str] = []
    operations = capabilities["operations"]
    modes = {mode["id"]: mode for mode in capabilities["proof_modes"]}

    for index, exchange in enumerate(exchanges):
        request = exchange["request"]
        response = exchange["response"]
        if response["operation"] != request["operation"]:
            continue

        operation = request["operation"]
        capability = operations[operation]
        unadvertised: str | None = None
        if not capability["supported"]:
            unadvertised = f"{operation} operation"

        params = request["params"]
        if unadvertised is None and operation in {"prepare", "prove"}:
            stage = params["stage"]
            if stage not in capability["stages"]:
                unadvertised = f"{operation}/{stage} stage"

        mode_id = params.get("proof_mode_id")
        mode: dict[str, Any] | None = None
        transformation: dict[str, Any] | None = None
        if (
            unadvertised is None
            and operation in {"execute", "prove", "verify"}
            and mode_id is not None
        ):
            mode = modes.get(mode_id)
            if mode is None:
                unadvertised = f"proof mode {mode_id}"

        if unadvertised is None and mode is not None:
            transformation_id = params.get("transformation_id")
            if transformation_id is not None:
                transformation = next(
                    (
                        item
                        for item in mode["transformations"]
                        if item["id"] == transformation_id
                    ),
                    None,
                )
                if transformation is None:
                    unadvertised = f"transformation {transformation_id}"

        if unadvertised is not None and response["status"] != "unsupported":
            issues.append(
                f"semantic /exchanges/{index}: unadvertised {unadvertised} "
                "must return unsupported"
            )
        if (
            unadvertised is None
            and operation == "prove"
            and response["status"] == "success"
            and mode is not None
        ):
            expected_format = mode["proof_format"]
            format_source = f"proof mode {mode['id']}"
            if params["stage"] == "transform" and transformation is not None:
                expected_format = transformation["output_format"]
                format_source = f"transformation {transformation['id']}"

            proof_id = response["result"]["proof_artifact_id"]
            for artifact_index, artifact in enumerate(response["artifacts"]):
                if (
                    artifact["id"] == proof_id
                    and artifact["media_type"] != expected_format
                ):
                    issues.append(
                        f"semantic /exchanges/{index}/response/artifacts/"
                        f"{artifact_index}/media_type: proof artifact must use "
                        f"{format_source} format {expected_format}"
                    )
    return issues


def _artifact_limit_issues(
    exchanges: list[dict[str, Any]],
    limits: dict[str, int],
    first_limited_exchange: int,
) -> list[str]:
    issues: list[str] = []
    for exchange_index in range(first_limited_exchange, len(exchanges)):
        exchange = exchanges[exchange_index]
        artifacts = exchange["response"]["artifacts"]
        if len(artifacts) > limits["max_artifact_count"]:
            issues.append(
                f"semantic /exchanges/{exchange_index}/response/artifacts: "
                f"count exceeds advertised limit {limits['max_artifact_count']}"
            )

        total_bytes = 0
        for artifact_index, artifact in enumerate(artifacts):
            byte_length = artifact["byte_length"]
            total_bytes += byte_length
            if byte_length > limits["max_artifact_bytes"]:
                issues.append(
                    f"semantic /exchanges/{exchange_index}/response/artifacts/"
                    f"{artifact_index}/byte_length: exceeds advertised limit "
                    f"{limits['max_artifact_bytes']}"
                )

        if total_bytes > limits["max_total_artifact_bytes"]:
            issues.append(
                f"semantic /exchanges/{exchange_index}/response/artifacts: "
                f"total byte length exceeds advertised limit "
                f"{limits['max_total_artifact_bytes']}"
            )
    return issues


def _metadata_identity_issues(
    exchanges: list[dict[str, Any]],
    adapter: dict[str, Any],
) -> list[str]:
    issues: list[str] = []
    for index, exchange in enumerate(exchanges):
        request = exchange["request"]
        response = exchange["response"]
        if (
            request["operation"] == "metadata"
            and response["operation"] == "metadata"
            and response["status"] == "success"
            and response["result"]["adapter"] != adapter
        ):
            issues.append(
                f"semantic /exchanges/{index}/response/result/adapter: "
                "does not match runtime capabilities"
            )
    return issues


def _catalog_semantic_issues(catalog: dict[str, Any]) -> list[str]:
    exchanges = catalog["exchanges"]
    issues: list[str] = []
    request_ids: set[str] = set()

    for index, exchange in enumerate(exchanges):
        exchange_issues = _exchange_semantic_issues(
            exchange["request"],
            exchange["response"],
        )
        issues.extend(
            f"{issue} (exchange {index}: {exchange['name']})"
            for issue in exchange_issues
        )
        request_id = exchange["request"]["request_id"]
        if request_id in request_ids:
            issues.append(
                f"semantic /exchanges/{index}/request/request_id: duplicate request ID"
            )
        request_ids.add(request_id)

    if not exchanges:
        issues.append("semantic /exchanges: capabilities exchange missing")
        return issues

    first = exchanges[0]
    if (
        first["request"]["operation"] != "capabilities"
        or first["response"]["operation"] != "capabilities"
        or first["response"]["status"] != "success"
    ):
        issues.append(
            "semantic /exchanges/0: first exchange must be successful capabilities"
        )

    capability_exchanges = [
        (index, exchange)
        for index, exchange in enumerate(exchanges)
        if exchange["request"]["operation"] == "capabilities"
        and exchange["response"]["operation"] == "capabilities"
        and exchange["response"]["status"] == "success"
    ]
    if not capability_exchanges:
        issues.append("semantic /exchanges: successful capabilities exchange missing")
        return issues

    capability_exchange_index, capability_exchange = capability_exchanges[0]
    capabilities = capability_exchange["response"]["result"]
    selected_version = capability_exchange["request"]["protocol_version"]
    host_versions = capability_exchange["request"]["params"]["host"][
        "supported_protocol_versions"
    ]
    manifest_versions = catalog["manifest"]["protocol_versions"]
    expected_version = select_protocol_version(host_versions, manifest_versions)

    if selected_version not in manifest_versions:
        issues.append(
            "semantic /manifest/protocol_versions: selected version is not advertised"
        )
    if selected_version not in capabilities["supported_protocol_versions"]:
        issues.append(
            "semantic /capabilities/supported_protocol_versions: "
            "selected version is not advertised"
        )
    if selected_version not in host_versions:
        issues.append(
            "semantic /exchanges/0/request/params/host/supported_protocol_versions: "
            "selected version is not supported by host"
        )
    if expected_version is None:
        issues.append(
            "semantic /manifest/protocol_versions: no version shared with host"
        )
    elif selected_version != expected_version:
        issues.append(
            "semantic /exchanges/0/request/protocol_version: "
            f"selected {selected_version}, expected highest exact {expected_version}"
        )
    for index, exchange in enumerate(exchanges):
        if exchange["request"]["protocol_version"] != selected_version:
            issues.append(
                f"semantic /exchanges/{index}/request/protocol_version: "
                "does not match selected version"
            )
    if catalog["cancellation"]["protocol_version"] != selected_version:
        issues.append(
            "semantic /cancellation/protocol_version: does not match selected version"
        )
    if catalog["cancellation"]["request_id"] not in request_ids:
        issues.append(
            "semantic /cancellation/request_id: unknown request ID"
        )

    if catalog["manifest"]["adapter_id"] != capabilities["adapter"]["id"]:
        issues.append(
            "semantic /manifest/adapter_id: does not match runtime capabilities"
        )

    issues.extend(_capability_issues(capabilities))
    issues.extend(_exchange_capability_issues(exchanges, capabilities))
    issues.extend(
        _artifact_limit_issues(
            exchanges,
            capabilities["limits"],
            capability_exchange_index + 1,
        )
    )
    issues.extend(
        _metadata_identity_issues(exchanges, capabilities["adapter"])
    )
    return issues


def validate_catalog(catalog: Any, schema: dict[str, Any]) -> list[str]:
    shape_issues = _catalog_shape_issues(catalog)
    if shape_issues:
        return shape_issues

    schema_issues = _schema_issues(catalog["manifest"], schema, "/manifest")
    schema_issues.extend(
        _schema_issues(catalog["cancellation"], schema, "/cancellation")
    )
    for index, exchange in enumerate(catalog["exchanges"]):
        schema_issues.extend(
            _schema_issues(
                exchange["request"],
                schema,
                f"/exchanges/{index}/request",
            )
        )
        schema_issues.extend(
            _schema_issues(
                exchange["response"],
                schema,
                f"/exchanges/{index}/response",
            )
        )
    if schema_issues:
        return schema_issues
    return _catalog_semantic_issues(catalog)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate zkperf Adapter Subprocess Protocol v1 catalogs."
    )
    parser.add_argument("catalogs", nargs="+", type=Path)
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    args = parser.parse_args()

    schema = json.loads(args.schema.read_text(encoding="utf-8"))
    Draft202012Validator.check_schema(schema)
    failed = False
    for path in args.catalogs:
        try:
            catalog = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            failed = True
            print(f"{path}: INVALID")
            print(f"  read: {error}")
            continue
        issues = validate_catalog(catalog, schema)
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
