# Supervised runner v1

Issue #11 implements one subprocess transaction in the core and connects it
to `zkperf run`. The existing protocol, plan, and immutable run directory are
the sources of truth. Reporting and resource measurements remain subsequent
issues; the command produces retained operation evidence and a run outcome.

## Transaction

- Invoke an absolute executable directly with literal arguments, an explicit
  environment (clear inheritance), and a fresh operation workspace.
- Validate and persist the request before spawning. Independently drain stdin,
  stdout, and stderr so a blocked channel cannot disable the watchdog.
- Retain up to 1 MiB of protocol stdout and 1 MiB of stderr by default. Drain
  excess logs without blocking; oversized stdout is a protocol failure.
- Time from immediately before spawn until process exit, pipe closure, and
  output readability. Exclude request construction, schema compilation,
  artifact hashing, evidence publication, and cancellation grace.
- Use the request deadline. Cancellation is a clonable token. Publish the
  protocol cancellation notice atomically; allow grace only when negotiated.
  Kill the Unix process group or Windows Job Object, including descendants
  whose parent has exited. Adapters must not detach (protocol section 4).
- Persist raw response bytes, bounded logs, exit code/signal, separate phase
  and cleanup durations, and protocol/process errors, including failed runs.
- Validate response framing, schema, correlation and artifact integrity before
  accepting outputs. Preserve structured error and unsupported statuses.
  Unreadable or corrupt diagnostic artifacts add errors without replacing an
  adapter's error or unsupported status. Successful responses require valid artifacts.

## CLI integration

Execute planned jobs sequentially with capability negotiation and metadata,
explicit preparation, execution, proving/transformation, and verification as
requested by the manifest and supported boundaries. Copy prior immutable
artifacts into each new operation's inputs. Never manufacture component times
from combined boundaries. Ctrl-C requests supervised cancellation. Retain the
run path on failure; dry-run and print-config remain side-effect free.

The current CLI supports separate phase boundaries and sequential execution.
Combined boundaries and end-to-end aggregates are rejected pending lifecycle
aggregation. Capabilities, metadata, and prerequisite operations without a
selected phase policy have a documented 30-second supervision limit and
one-second cancellation grace; selected phases use their manifest policies.
The environment is exactly `run.resources.environment_variables`; adapters
requiring PATH, HOME, temporary directories, or platform variables must list
them there. No implicit shell or inherited environment is used.

Operation workspaces live at `attempts/<attempt>/outputs/<request UUID>/`, with
their own `inputs`, `outputs`, and `control` roots. Evidence lives outside that
workspace at `logs/<attempt UUID>/<request UUID>/`: `request.json`, `stdout.bin`,
`stderr.bin`, and `outcome.json`. Logs and accepted output files also receive
immutable artifact snapshots. Raw partial output stays diagnostic on failure.
`phase_duration_ns` measures the subprocess boundary using the host monotonic
clock; `cleanup_duration_ns` is separate. Polling resolution is two milliseconds.

Artifact IDs belong to their response, not to the run. The host assigns each
retained output an identity tied to its producing operation before combining
inputs from different responses. Preparation outputs remain available to
initial proving and proof transformations alongside the trace or prior proof.
Proof media types must match the negotiated initial or transformation format
before the next stage can consume them.

Preparation follows phase dependencies: build needs environment preparation;
setup also needs build; execution and later phases need the advertised setup
stage. A build-only job stops after build, and a proving-only job stops after
the initial proof. Verification consumes the mode's full transformation chain.
Advertised limits outside the host's supported integer range fail negotiation
with a retained error and a finalized failed run.

Execution currently retains requested resource configuration without enforcing
CPU/memory/network limits or collecting resource measurements. It does not
produce statistical summaries or claim a completed BenchmarkReport.

## Verification

Targeted subprocess fixtures cover success, nonzero exit, signal exit,
malformed/correlated output, stdout overflow, stderr flooding, blocked stdin,
timeout, graceful and forced cancellation, and inherited pipes/descendants.
CLI tests cover real invocation, retained failures, and existing dry-run
behavior. Run rustfmt, compilation, Clippy, dependency-policy checks and these
targeted tests; check the declared Rust 1.85 MSRV where installed.
