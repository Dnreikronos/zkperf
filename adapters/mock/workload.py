"""Deterministic SHA-256 workload and deliberately non-cryptographic proofs."""

import hashlib
import json
import zlib

from protocol import (
    COMPRESSED_FORMAT,
    MAX_ARTIFACT_BYTES,
    PROOF_FORMAT,
    Failure,
    artifact,
    digest,
    encode,
)


def canonical_output(data):
    return hashlib.sha256(data).hexdigest().encode("ascii") + b"\n"


def statement(benchmark, data):
    output = canonical_output(data)
    output_digest = digest(output)
    if output_digest != benchmark["expected_output_digest"]:
        raise Failure("output_mismatch", "Computed workload output differs from the benchmark.")
    commitments = {key: value for key, value in
                   (("input", digest(data)), ("output", output_digest))
                   if benchmark["commitment_policy"][key]}
    return {"benchmark": benchmark, "output_digest": output_digest,
            "commitment_digests": commitments}


def one_input(inputs, kind):
    values = [(entry, data) for entry, data in inputs.values() if entry["kind"] == kind]
    if len(values) != 1:
        raise Failure("invalid_input", "Expected exactly one " + kind + " artifact.", phase="artifact")
    return values[0]


def seal(payload):
    return digest(b"zkperf-mock-proof-v1\0" + encode(payload))


def unpack_proof(entry, data):
    if entry["media_type"] == COMPRESSED_FORMAT:
        decoder = zlib.decompressobj()
        data = decoder.decompress(data, MAX_ARTIFACT_BYTES + 1)
        if len(data) > MAX_ARTIFACT_BYTES or not decoder.eof or decoder.unused_data:
            raise Failure("invalid_proof", "Invalid or oversized compressed mock proof.")
    elif entry["media_type"] != PROOF_FORMAT:
        raise Failure("invalid_proof", "Unknown mock proof format.")
    proof = json.loads(data)
    payload = proof["payload"]
    if proof["seal"] != seal(payload) or payload["format"] != "mock-proof-v1":
        raise Failure("invalid_proof", "Mock proof seal is invalid.")
    claimed = payload["statement"]
    actual = statement(claimed["benchmark"], bytes.fromhex(payload["input_hex"]))
    if actual != claimed:
        raise Failure("invalid_proof", "Mock proof does not describe its witness.")
    return proof, actual


def run(request, response, inputs):
    params, operation = request["params"], request["operation"]
    benchmark = params["benchmark"]
    mode = params.get("proof_mode_id")
    if mode is not None and mode != "mock-core":
        raise Failure("unsupported_proof_mode", "Unknown mock proof mode.", "unsupported")
    if operation == "prepare":
        stage = params["stage"]
        kinds = {"environment": "parameters", "build": "guest_program", "setup": "proving_key"}
        if stage not in kinds:
            raise Failure("unsupported_stage", "Unknown preparation stage.", "unsupported")
        name = artifact(request, response, "prepared-" + stage, kinds[stage],
                        encode({"stage": stage, "benchmark": benchmark, "format": "mock-prepared-v1"}))
        return {"stage": stage, "prepared_artifact_ids": [name]}
    if operation == "execute":
        _, data = one_input(inputs, "canonical_input")
        claimed = statement(benchmark, data)
        output = artifact(request, response, "canonical-output", "canonical_output", canonical_output(data))
        trace = artifact(request, response, "execution-trace", "execution_trace",
                         encode({"benchmark": benchmark, "input_hex": data.hex()}))
        return {"canonical_output_artifact_id": output, "execution_artifact_id": trace,
                "commitment_digests": claimed["commitment_digests"]}
    if operation == "prove":
        stage = params["stage"]
        if stage == "initial":
            _, data = one_input(inputs, "execution_trace")
            trace = json.loads(data)
            if trace["benchmark"] != benchmark:
                raise Failure("statement_mismatch", "Trace belongs to a different benchmark.")
            claimed = statement(benchmark, bytes.fromhex(trace["input_hex"]))
            payload = {"format": "mock-proof-v1", "statement": claimed, "input_hex": trace["input_hex"]}
            proof = encode({"payload": payload, "seal": seal(payload)})
            proof_id = artifact(request, response, "raw-proof", "proof", proof, PROOF_FORMAT)
            public_id = artifact(request, response, "public-values", "public_values", encode(claimed))
            return {"stage": stage, "proof_mode_id": mode, "proof_artifact_id": proof_id,
                    "public_values_artifact_id": public_id,
                    "commitment_digests": claimed["commitment_digests"]}
        if stage != "transform" or params["transformation_id"] != "mock-compress":
            raise Failure("unsupported_transformation", "Unknown mock transformation.", "unsupported")
        entry, data = one_input(inputs, "proof")
        proof, claimed = unpack_proof(entry, data)
        if entry["media_type"] != PROOF_FORMAT or claimed["benchmark"] != benchmark:
            raise Failure("statement_mismatch", "Transformation requires an initial proof of this benchmark.")
        proof_id = artifact(request, response, "compressed-proof", "proof",
                            zlib.compress(encode(proof), level=9), COMPRESSED_FORMAT)
        return {"stage": stage, "proof_mode_id": mode, "transformation_id": "mock-compress",
                "proof_artifact_id": proof_id, "commitment_digests": claimed["commitment_digests"]}
    entry, data = one_input(inputs, "proof")
    _, claimed = unpack_proof(entry, data)
    commitments = claimed["commitment_digests"]
    application_statement = {"workload": benchmark["case_id"],
                             "input_commitment": commitments.get("input", {}).get("value"),
                             "output_commitment": commitments.get("output", {}).get("value")}
    if (claimed["benchmark"] != benchmark
            or claimed["output_digest"] != params["expected_output_digest"]
            or commitments != params["expected_commitment_digests"]
            or params["statement"] != application_statement):
        raise Failure("verification_failed", "Proof does not match the complete application statement.")
    return {"verdict": "accepted", "output_digest": claimed["output_digest"],
            "commitment_digests": commitments}
