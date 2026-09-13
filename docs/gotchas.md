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
- Secret-key checks need known credential aliases as well as words like token
  and password. Keep manifest validation and saved metadata on the same check;
  test case and separator variants inside nested objects and arrays.
- Published schema versions are immutable. Allowing objects in scalar fields
  needs a new report version, even when all older examples still validate.
- Start subprocess deadline assertions after fixture setup. Run creation collects
  host metadata and hashes the executable; including that work makes supervisor
  tests depend on CI machine speed and debug-binary size. For subprocess tests
  that check for blocking I/O, signal readiness after setup and give setup its
  own timeout so a slow fixture is not reported as a blocked read.
- Linux process I/O includes waited-for children. Summing `/proc/PID/io` with
  retained descendant counters double-counts work after reap. Sample per-thread
  I/O and test a writing grandchild whose parent and grandparent both wait.
- Thread identity checks do not connect a proc read to an earlier process
  snapshot. Validate the opened process's start time against the snapshot and
  open tasks through that same directory handle before retaining I/O counters.
- Counter units are not precision guarantees. Preserve sampled peak semantics,
  partial tree coverage, identity resolution and unavailable-read reasons with
  the values, including failed operations.
- A failed worker's missing diagnostics are not measured zeros. Keep unavailable
  counts separate from a collector that is known not to have started.
- A timestamp sent through a channel can precede its delivery. Publish stop
  boundaries synchronously before waking workers and test delayed notifications.
- Sampler startup must not hide child completion. Use non-reaping exit checks
  while the process identity is being captured, and stop sampling before cleanup
  can release the root PID.
