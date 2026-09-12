# Execution gotchas

- Protocol artifact IDs are message-local. Keep producer identity when saving
  outputs, and assign unique artifact IDs when combining them with fixtures or
  outputs from other operations.
- Fresh workspaces need every dependency staged explicitly. Test proving with
  a setup key, and test phase subsets with an adapter that rejects extra work.
- Schema-valid JSON integers can exceed Rust integer ranges. Convert external
  limits fallibly and test that failures finalize the run rather than panic.
- Artifact integrity does not prove capability agreement. Check each proof's
  media type against its negotiated mode or transformation before consuming it.
- Diagnostic artifact failures must not erase a structured adapter failure.
  Test both unreadable files and hash mismatches; they fail at different points
  in supervision and evidence storage.
- Runtime metadata must use the configuration after defaults and CLI overrides.
  Keep that resolved configuration before filtering settings for an operation;
  otherwise metadata can lose settings targeted at a later stage. Test both
  active overrides and overrides intended for another stage.
