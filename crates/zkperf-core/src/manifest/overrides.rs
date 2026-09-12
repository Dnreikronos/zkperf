use std::num::NonZeroU64;
use std::path::PathBuf;

use super::{BenchmarkManifest, ManifestError, OutputFormat, load};

/// Explicit replacements for a validated manifest's command-line settings.
///
/// Unspecified fields retain the manifest value. Output paths obey the same
/// manifest-relative rules as the source file; no policy defaults are added.
#[derive(Clone, Debug, Default)]
pub struct ManifestOverrides {
    pub warmups: Option<u64>,
    pub runs: Option<NonZeroU64>,
    pub output_directory: Option<PathBuf>,
    pub output_formats: Option<Vec<OutputFormat>>,
}

impl BenchmarkManifest {
    /// Applies explicit replacements while preserving manifest invariants.
    ///
    /// # Errors
    ///
    /// Returns a field-addressed error for invalid output paths or formats.
    pub fn with_overrides(mut self, overrides: ManifestOverrides) -> Result<Self, ManifestError> {
        if let Some(warmups) = overrides.warmups {
            self.run.warmups = warmups;
        }
        if let Some(runs) = overrides.runs {
            self.run.runs = runs;
        }
        if let Some(directory) = overrides.output_directory {
            let base = self.manifest_path.parent().ok_or_else(|| {
                ManifestError::new("manifest", "manifest has no parent directory")
            })?;
            self.outputs.directory = load::resolve_output_directory(base, &directory)?;
        }
        if let Some(formats) = overrides.output_formats {
            if formats.is_empty() {
                return Err(ManifestError::new(
                    "outputs.formats",
                    "must contain at least one item",
                ));
            }
            for (index, format) in formats.iter().enumerate() {
                if formats[..index].contains(format) {
                    return Err(ManifestError::new(
                        format!("outputs.formats[{index}]"),
                        "duplicate value",
                    ));
                }
            }
            self.outputs.formats = formats;
        }
        Ok(self)
    }
}
