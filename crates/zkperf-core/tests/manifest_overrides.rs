use std::num::NonZeroU64;
use std::path::Path;

use zkperf_core::{BenchmarkManifest, ManifestOverrides, OutputFormat};

fn manifest() -> BenchmarkManifest {
    BenchmarkManifest::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/manifest/zkperf.toml"),
    )
    .expect("valid fixture")
}

#[test]
fn overrides_update_the_validated_definition_without_changing_other_policy() {
    let original = manifest();
    assert_eq!(
        original
            .clone()
            .with_overrides(ManifestOverrides::default())
            .unwrap(),
        original
    );
    let effective = original
        .clone()
        .with_overrides(ManifestOverrides {
            warmups: Some(0),
            runs: NonZeroU64::new(9),
            output_directory: Some("cli-results".into()),
            output_formats: Some(vec![OutputFormat::Csv]),
        })
        .unwrap();
    assert_eq!(effective.run().warmups(), 0);
    assert_eq!(effective.run().runs().get(), 9);
    assert_eq!(effective.outputs().formats(), &[OutputFormat::Csv]);
    assert_eq!(
        effective.outputs().directory(),
        original
            .manifest_path()
            .parent()
            .unwrap()
            .join("cli-results")
    );
    assert_eq!(effective.run().policy(), original.run().policy());
    assert_eq!(effective.run().timeouts(), original.run().timeouts());
    assert_eq!(effective.workloads(), original.workloads());
    assert_eq!(effective.engines(), original.engines());
    assert_eq!(manifest(), original);
}

#[test]
fn overrides_preserve_output_invariants() {
    for directory in ["", "../escape", "workload.md"] {
        let error = manifest()
            .with_overrides(ManifestOverrides {
                output_directory: Some(directory.into()),
                ..ManifestOverrides::default()
            })
            .unwrap_err();
        assert_eq!(error.field_path(), "outputs.directory");
    }
    for formats in [vec![], vec![OutputFormat::Json, OutputFormat::Json]] {
        let error = manifest()
            .with_overrides(ManifestOverrides {
                output_formats: Some(formats),
                ..ManifestOverrides::default()
            })
            .unwrap_err();
        assert!(error.field_path().starts_with("outputs.formats"));
    }
}
