use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{BenchmarkManifest, FixtureHashError, Sha256Digest};

/// Content identity of one resolved file used by a plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FileProvenance {
    path: PathBuf,
    sha256: Sha256Digest,
}

impl FileProvenance {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}

pub(super) fn collect(
    manifest: &BenchmarkManifest,
) -> Result<Vec<FileProvenance>, FixtureHashError> {
    let mut files = BTreeMap::new();
    for engine in manifest.engines() {
        files.insert(engine.adapter().path(), engine.adapter());
    }
    for workload in manifest.workloads() {
        files.insert(workload.specification().path(), workload.specification());
        for input in workload.inputs() {
            files.insert(input.fixture().path(), input.fixture());
            files.insert(input.expected_output().path(), input.expected_output());
        }
    }
    files
        .into_iter()
        .map(|(path, file)| {
            Ok(FileProvenance {
                path: path.to_owned(),
                sha256: file.sha256()?,
            })
        })
        .collect()
}
