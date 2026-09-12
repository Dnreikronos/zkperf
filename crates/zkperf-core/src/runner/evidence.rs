use std::fs::File;
use std::io::Write;
use std::path::Path;

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde_json::{Value, json};

use super::{AdapterInvocation, OperationRecord, wire::invalid};
use crate::{
    Artifact, ArtifactKind, ArtifactRequest, NonEmptyString, RunDirectory, RunError, RunWorkspace,
};

pub(super) struct Evidence {
    pub stdout: File,
    pub stderr: File,
    pub control: Dir,
}

fn descriptor(workspace: &RunWorkspace, id: &str, name: &str) -> ArtifactRequest {
    ArtifactRequest {
        path: format!("logs/{}/{id}/{name}", workspace.attempt_id()),
        name: NonEmptyString::new(name).unwrap(),
        kind: ArtifactKind::Log,
        media_type: if name.ends_with("json") {
            "application/json"
        } else {
            "application/octet-stream"
        }
        .into(),
        attempt_id: Some(workspace.attempt_id()),
    }
}

impl Evidence {
    pub(super) fn create(
        run: &mut RunDirectory,
        workspace: &RunWorkspace,
        invocation: &AdapterInvocation,
        root: &Path,
    ) -> Result<Self, RunError> {
        let id = invocation.request["request_id"].as_str().unwrap();
        // The request is an exclusive reservation: duplicate invocations never spawn.
        run.store(
            &descriptor(workspace, id, "request.json"),
            &serde_json::to_vec_pretty(&invocation.request)?,
        )?;
        let parent = Dir::open_ambient_dir(workspace.outputs(), ambient_authority())
            .map_err(|error| invalid(&error.to_string()))?;
        parent
            .create_dir_all(id)
            .map_err(|error| invalid(&error.to_string()))?;
        let directory = parent
            .open_dir_nofollow(id)
            .map_err(|error| invalid(&error.to_string()))?;
        for child in ["inputs", "outputs", "control"] {
            directory
                .create_dir_all(child)
                .map_err(|error| invalid(&error.to_string()))?;
            directory
                .open_dir_nofollow(child)
                .map_err(|error| invalid(&error.to_string()))?;
        }
        if !root.is_dir() {
            return Err(invalid("operation root is not a directory"));
        }
        Ok(Self {
            stdout: run.open_new(&descriptor(workspace, id, "stdout.bin").path)?,
            stderr: run.open_new(&descriptor(workspace, id, "stderr.bin").path)?,
            control: directory
                .open_dir_nofollow("control")
                .map_err(|error| invalid(&error.to_string()))?,
        })
    }
}

pub(super) fn cancel(control: &Dir, request: &Value, reason: &str) -> std::io::Result<()> {
    let contents = serde_json::to_vec(&json!({
        "protocol": request["protocol"], "protocol_version": request["protocol_version"],
        "request_id": request["request_id"], "reason": reason,
    }))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = control.open_with("cancel.tmp", &options)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    drop(file);
    control.hard_link("cancel.tmp", control, "cancel.json")?;
    control.remove_file("cancel.tmp")
}

pub(super) fn finish(
    run: &mut RunDirectory,
    workspace: &RunWorkspace,
    record: &OperationRecord,
) -> Result<(), RunError> {
    // Publish the outcome before hashing logs, retaining it even if adoption fails.
    run.store(
        &descriptor(workspace, &record.request_id, "outcome.json"),
        &serde_json::to_vec_pretty(record)?,
    )?;
    for name in ["stdout.bin", "stderr.bin"] {
        run.adopt(&descriptor(workspace, &record.request_id, name))?;
    }
    Ok(())
}

pub(super) fn artifacts(
    run: &mut RunDirectory,
    workspace: &RunWorkspace,
    invocation: &AdapterInvocation,
    response: &Value,
) -> Result<Vec<Artifact>, RunError> {
    let entries = response["artifacts"].as_array().unwrap();
    let id = invocation.request["request_id"].as_str().unwrap();
    let mut artifacts = Vec::new();
    for entry in entries {
        let path = entry["path"].as_str().unwrap();
        let artifact = run.adopt(&ArtifactRequest {
            path: workspace.output_path(&format!("{id}/{path}")),
            name: NonEmptyString::new(entry["id"].as_str().unwrap()).unwrap(),
            kind: match entry["kind"].as_str().unwrap() {
                "proof" => ArtifactKind::Proof,
                _ => ArtifactKind::Other,
            },
            media_type: entry["media_type"].as_str().unwrap().into(),
            attempt_id: Some(workspace.attempt_id()),
        })?;
        let actual = serde_json::to_value(&artifact)?;
        if actual["digest"] != entry["digest"] || actual["byte_length"] != entry["byte_length"] {
            return Err(invalid("artifact length or digest mismatch"));
        }
        artifacts.push(artifact);
    }
    Ok(artifacts)
}

fn open_artifact(root: &Path, relative: &str) -> Result<cap_std::fs::File, RunError> {
    let mut directory = Dir::open_ambient_dir(root, ambient_authority())
        .map_err(|error| invalid(&error.to_string()))?;
    let parts: Vec<_> = relative.split('/').collect();
    if parts.first() != Some(&"outputs")
        || parts.len() < 2
        || parts.iter().any(|part| {
            part.is_empty() || *part == "." || *part == ".." || part.contains(['\\', ':', '\0'])
        })
    {
        return Err(invalid("invalid output artifact path"));
    }
    for part in &parts[..parts.len() - 1] {
        directory = directory
            .open_dir_nofollow(part)
            .map_err(|error| invalid(&error.to_string()))?;
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    directory
        .open_with(parts.last().unwrap(), &options)
        .map_err(|error| invalid(&error.to_string()))
}

// Only framing and file readability belong before the timing boundary. Schema
// validation and content hashing happen after the supervised transaction.
pub(super) fn readable(
    root: &Path,
    bytes: &[u8],
    limits: super::RunnerLimits,
) -> Result<(), RunError> {
    let Ok(response) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(());
    };
    let Some(entries) = response["artifacts"].as_array() else {
        return Ok(());
    };
    if entries.len() as u64 > limits.artifact_count {
        return Err(invalid("artifact count exceeds limit"));
    }
    let mut total = 0_u64;
    for entry in entries {
        let length = entry["byte_length"]
            .as_u64()
            .ok_or_else(|| invalid("artifact length missing"))?;
        total = total
            .checked_add(length)
            .ok_or_else(|| invalid("artifact total overflow"))?;
        if length > limits.artifact_bytes || total > limits.total_artifact_bytes {
            return Err(invalid("artifact bytes exceed limit"));
        }
        let file = open_artifact(
            root,
            entry["path"]
                .as_str()
                .ok_or_else(|| invalid("artifact path missing"))?,
        )?;
        let actual = file
            .metadata()
            .map_err(|error| invalid(&error.to_string()))?;
        if !actual.is_file() || actual.len() != length {
            return Err(invalid(
                "artifact is not a regular file of the declared length",
            ));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn cancellation_uses_the_original_control_directory_after_replacement() {
        let root = std::env::temp_dir().join(format!(
            "zkperf-control-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("control")).unwrap();
        std::fs::create_dir(root.join("outside")).unwrap();
        let control = Dir::open_ambient_dir(root.join("control"), ambient_authority()).unwrap();
        std::fs::rename(root.join("control"), root.join("retained")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("control")).unwrap();
        cancel(
            &control,
            &json!({"protocol":"zkperf-adapter","protocol_version":"1.0.0",
            "request_id":"10000000-0000-4000-8000-000000000001"}),
            "user_cancelled",
        )
        .unwrap();
        assert!(root.join("retained/cancel.json").is_file());
        assert!(!root.join("outside/cancel.json").exists());
        drop(control);
        std::fs::remove_dir_all(root).unwrap();
    }
}
