use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::runner::wire::{Validator, invalid};
use crate::{
    AdapterInvocation, ArtifactKind, ArtifactRequest, BenchmarkPlan, CancellationToken,
    ManifestEngine, ManifestWorkload, NonEmptyString, OperationOutcome, PlannedJob, RunDirectory,
    RunError, RunWorkspace, RunnerLimits, run_operation,
};

pub(super) struct InvocationResult {
    pub result: Value,
    artifacts: BTreeMap<String, Value>,
}

impl InvocationResult {
    pub fn artifact(&self, id: &Value) -> Result<Value, RunError> {
        id.as_str()
            .and_then(|id| self.artifacts.get(id))
            .cloned()
            .ok_or_else(|| invalid("artifact reference missing from producing response"))
    }
}

pub(super) struct Session<'a> {
    pub plan: &'a BenchmarkPlan,
    pub job: &'a PlannedJob,
    pub workload: &'a ManifestWorkload,
    pub engine: &'a ManifestEngine,
    pub capabilities: Value,
    pub benchmark: Value,
    pub canonical_input: Value,
    run: &'a mut RunDirectory,
    workspace: RunWorkspace,
    invocation: AdapterInvocation,
    cancellation: &'a CancellationToken,
    sources: BTreeMap<String, std::path::PathBuf>,
    sequence: u64,
    adapter_id: String,
}

impl<'a> Session<'a> {
    pub fn new(
        run: &'a mut RunDirectory,
        plan: &'a BenchmarkPlan,
        job: &'a PlannedJob,
        cancellation: &'a CancellationToken,
    ) -> Result<Self, RunError> {
        let engine = plan
            .manifest()
            .engines()
            .iter()
            .find(|engine| engine.id() == job.engine_id())
            .ok_or_else(|| invalid("planned engine missing"))?;
        let workload = plan
            .manifest()
            .workloads()
            .iter()
            .find(|workload| workload.id() == job.workload_id())
            .ok_or_else(|| invalid("planned workload missing"))?;
        let input = workload
            .inputs()
            .iter()
            .find(|input| input.id() == job.input_id())
            .ok_or_else(|| invalid("planned input missing"))?;
        let adapter: Value = serde_json::from_slice(
            &fs::read(engine.adapter().path()).map_err(|error| invalid(&error.to_string()))?,
        )?;
        Validator::new()?.validate(&adapter)?;
        if adapter["kind"] != "zkperf-adapter-manifest" {
            return Err(invalid("expected adapter manifest"));
        }
        if !adapter["protocol_versions"]
            .as_array()
            .unwrap()
            .contains(&json!("1.0.0"))
        {
            return Err(invalid("no compatible adapter protocol version"));
        }
        let command = adapter["command"].as_array().unwrap();
        let executable = engine
            .adapter()
            .path()
            .parent()
            .unwrap()
            .join(command[0].as_str().unwrap());
        let environment = plan
            .manifest()
            .run()
            .resources()
            .environment_variables()
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();
        let workspace = run.attempt(job, 0)?;
        let mut session = Self {
            plan,
            job,
            workload,
            engine,
            run,
            workspace,
            cancellation,
            capabilities: Value::Null,
            benchmark: Value::Null,
            sources: BTreeMap::new(),
            canonical_input: Value::Null,
            sequence: 0,
            adapter_id: adapter["adapter_id"].as_str().unwrap().into(),
            invocation: AdapterInvocation {
                executable,
                arguments: command[1..]
                    .iter()
                    .map(|arg| OsString::from(arg.as_str().unwrap()))
                    .collect(),
                environment,
                request: Value::Null,
                limits: RunnerLimits::default(),
                graceful_cancellation: false,
            },
        };
        session.canonical_input =
            session.fixture("canonical-input", "canonical_input", input.fixture().path())?;
        let expected = session.fixture(
            "expected-output",
            "canonical_output",
            input.expected_output().path(),
        )?;
        let specification = session.fixture(
            "workload-specification",
            "source",
            workload.specification().path(),
        )?;
        session.benchmark = json!({"case_id": format!("{}-{}", workload.id().as_str(), input.id().as_str()),
            "workload_revision": workload.revision(), "workload_digest": specification["digest"],
            "implementation_lane": workload.implementation_lane(), "security_target_bits": workload.security_target_bits(),
            "expected_output_digest": expected["digest"],
            "commitment_policy": {"input": input.commit_input() != crate::Commitment::None,
                "output": input.commit_output() != crate::Commitment::None}});
        Ok(session)
    }

    fn fixture(&mut self, id: &str, kind: &str, path: &Path) -> Result<Value, RunError> {
        let bytes = fs::read(path).map_err(|error| invalid(&error.to_string()))?;
        let snapshot = self.run.store(
            &ArtifactRequest {
                path: self.workspace.input_path(id),
                name: NonEmptyString::new(id).unwrap(),
                kind: ArtifactKind::Other,
                media_type: "application/octet-stream".into(),
                attempt_id: Some(self.workspace.attempt_id()),
            },
            &bytes,
        )?;
        let value = serde_json::to_value(snapshot)?;
        self.sources.insert(
            id.into(),
            self.run.path().join(value["uri"].as_str().unwrap()),
        );
        Ok(
            json!({"id":id,"kind":kind,"path":format!("inputs/{id}"),"media_type":"application/octet-stream",
            "byte_length": value["byte_length"], "digest":value["digest"]}),
        )
    }

    pub fn invoke(
        &mut self,
        operation: &str,
        params: Value,
        phase: &str,
    ) -> Result<InvocationResult, RunError> {
        let digest = crate::digest::hash_bytes(
            format!("{}:{}", self.workspace.attempt_id(), self.sequence).as_bytes(),
        );
        self.sequence += 1;
        let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
        bytes[6] = (bytes[6] & 15) | 128;
        bytes[8] = (bytes[8] & 63) | 128;
        let id = uuid::Uuid::from_bytes(bytes).to_string();
        self.stage_inputs(&id, &params)?;
        let policy = self
            .plan
            .manifest()
            .run()
            .timeouts()
            .iter()
            .find(|timeout| {
                serde_json::to_value(timeout.phase()).ok().as_ref() == Some(&json!(phase))
            });
        let (limit_ms, grace_ms) = policy.map_or((30_000, 1_000), |policy| {
            (policy.limit_ms().get(), policy.termination_grace_ms())
        });
        self.invocation.request = json!({"protocol":"zkperf-adapter","protocol_version":"1.0.0","request_id":id,
            "operation":operation,"workspace":{"inputs_dir":"inputs","outputs_dir":"outputs","control_dir":"control"},
            "timeout":{"limit_ns":limit_ms.checked_mul(1_000_000).ok_or_else(|| invalid("timeout overflow"))?,
                "termination_grace_ns":grace_ms.checked_mul(1_000_000).ok_or_else(|| invalid("grace overflow"))?},"params":null});
        self.invocation.request["params"] = params;
        let result = run_operation(
            self.run,
            &self.workspace,
            &self.invocation,
            self.cancellation,
        )?;
        if result.record.outcome != OperationOutcome::Success {
            return Err(invalid(&format!(
                "{operation}: {:?}; {}",
                result.record.outcome,
                result.record.errors.join("; ")
            )));
        }
        let response = result.response.unwrap();
        let mut artifacts = BTreeMap::new();
        for (index, (entry, snapshot)) in response["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .zip(result.artifacts)
            .enumerate()
        {
            let response_id = entry["id"].as_str().unwrap();
            // Wire IDs are response-local; downstream requests use producer-scoped IDs.
            let input_id = format!("{id}-{index}");
            let snapshot = serde_json::to_value(snapshot)?;
            self.sources.insert(
                input_id.clone(),
                self.run.path().join(snapshot["uri"].as_str().unwrap()),
            );
            let mut entry = entry.clone();
            entry["path"] = format!("inputs/{input_id}").into();
            entry["id"] = input_id.into();
            artifacts.insert(response_id.into(), entry);
        }
        Ok(InvocationResult {
            result: response["result"].clone(),
            artifacts,
        })
    }

    fn stage_inputs(&self, id: &str, params: &Value) -> Result<(), RunError> {
        for entry in super::capabilities::input_artifacts(params) {
            let key = entry["id"]
                .as_str()
                .ok_or_else(|| invalid("input artifact ID missing"))?;
            let source = self
                .sources
                .get(key)
                .ok_or_else(|| invalid("input artifact source missing"))?;
            let relative = self.workspace.output_path(&format!("{id}/inputs/{key}"));
            let mut writer = self.run.open_new(&relative)?;
            std::io::copy(
                &mut fs::File::open(source).map_err(|error| invalid(&error.to_string()))?,
                &mut writer,
            )
            .map_err(|error| invalid(&error.to_string()))?;
        }
        Ok(())
    }

    pub fn negotiate(&mut self) -> Result<(), RunError> {
        let capabilities = self.invoke("capabilities", json!({"host":{"name":"zkperf","version":env!("CARGO_PKG_VERSION"),"supported_protocol_versions":["1.0.0"]}}), "capabilities")?;
        let capabilities = capabilities.result;
        super::capabilities::validate(&capabilities, &self.adapter_id)?;
        let limits = &capabilities["limits"];
        let limit = |name: &str| {
            limits[name].as_u64().ok_or_else(|| {
                invalid(&format!(
                    "capability limit {name} is outside the supported u64 integer range"
                ))
            })
        };
        self.invocation.limits.stdout_bytes = self
            .invocation
            .limits
            .stdout_bytes
            .min(usize::try_from(limit("max_protocol_stdout_bytes")?).unwrap_or(usize::MAX));
        self.invocation.limits.artifact_count = self
            .invocation
            .limits
            .artifact_count
            .min(limit("max_artifact_count")?);
        self.invocation.limits.artifact_bytes = self
            .invocation
            .limits
            .artifact_bytes
            .min(limit("max_artifact_bytes")?);
        self.invocation.limits.total_artifact_bytes = self
            .invocation
            .limits
            .total_artifact_bytes
            .min(limit("max_total_artifact_bytes")?);
        self.invocation.graceful_cancellation = capabilities["cancellation"]["graceful"] == true;
        let metadata = self.invoke(
            "metadata",
            json!({"configuration":self.engine.configuration(),"artifacts":[]}),
            "metadata",
        )?;
        if metadata.result["adapter"] != capabilities["adapter"] {
            return Err(invalid(
                "metadata adapter identity differs from capabilities",
            ));
        }
        self.capabilities = capabilities;
        Ok(())
    }
}
