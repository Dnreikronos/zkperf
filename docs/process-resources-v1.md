# Process resource evidence v1

Issue #14 adds `resources` to each operation's `outcome.json`, including failed,
timed-out and cancelled operations. This is operation evidence for the report
producer in issue #16; published BenchmarkReport schemas remain unchanged.

## Contract

- Wall time remains `phase_duration_ns`, measured by the runner's monotonic
  clock. Its existing subprocess/output-readability boundary and two-millisecond
  supervision polling interval remain unchanged. Cleanup and cancellation grace
  are excluded. Nanoseconds are a storage unit, not a claim of clock accuracy.
- A separate worker samples the adapter and discoverable descendants on Linux,
  macOS and Windows using sysinfo 0.36.1. The collector itself is excluded.
  Unsupported hosts retain wall timing and explicit unavailable resource values.
- CPU time is the sum of the largest observed cumulative CPU counter for each
  discovered process, converted from milliseconds to nanoseconds. macOS and
  Windows I/O sums per-process cumulative byte counters. Linux I/O instead sums
  `/proc/PID/task/TID/io` counters by thread identity: `/proc/PID/io` includes
  waited-for children and would count retained child I/O twice. Exited processes
  and threads retain their last observations. Short-lived threads and their final
  counter increments can be missed. Thread identity includes native start ticks;
  open proc directory handles and before/after identity reads reject read races.
  Before reading tasks, the collector checks the opened process's start time
  against its snapshot identity, converting proc clock ticks to sysinfo's whole
  Unix seconds. It opens tasks relative to that same process directory so PID
  reuse cannot redirect the reads after validation. An identity mismatch or
  unreadable identity records an I/O read failure and retains only earlier data.
- RSS is the maximum of the sum of resident bytes in a sampling sweep. It is
  neither an exact high-water mark nor the sum of individual lifetime peaks.
  Reads within a sweep are not simultaneous; shared pages can be counted twice.
- Every capture discloses its backend, units, precision limits, sampling count,
  interval, largest actual gap, collection cost, and process tracking limits.
  CPU and I/O are incomplete lifetime observations; unsampled processes, final
  counter increments, transient memory peaks and already-reparented descendants
  can be missed. Unknown values use explicit reasons, never fabricated zeros.
- Collection diagnostics are an observed value. A worker that cannot start or
  panics reports `collection.availability = "unavailable"` with a
  `collector_failed` reason; its lost counts and durations are not replaced by
  zeros. Operations that never start a collector retain known zero diagnostics.
- Identity uses PID plus the OS start time exposed by sysinfo (whole seconds).
  On Linux and macOS, the watchdog uses a non-reaping exit check while the worker
  anchors the adapter identity. A completed child can finish without waiting for
  that read; sampling is stopped before cleanup reaps the root. Windows keeps
  the child process handle open through the same boundary. Cancellation and
  deadlines continue to be checked during startup.
  A disappeared identity is retired and cannot be resurrected; a changed start
  time never inherits counters or membership. A parent must be present with its
  tracked identity to discover new children. Reparented children already observed
  remain tracked. PID reuse entirely between samples in the same second cannot
  be distinguished and is disclosed as an identity precision limitation.
- Fresh snapshots avoid retaining stale successful reads after later failures.
  The backend can return zero for inaccessible counters; all-zero observations
  are unavailable rather than evidence of no CPU, memory, or I/O usage. Their gap
  messages name the affected metric. A root that was never observed keeps the
  `not_observed` reason, even if a sweep found other processes.

## Collector budget

The worker runs at most once per 50 milliseconds, waiting at least 19 times its
last collection duration between sweeps (a 5% amortized worker wall-time budget).
It skips missed intervals instead of catching up. Retained process identities
and Linux I/O thread identities are each capped at 4096; each Linux I/O sweep also
visits at most 4096 tasks. Limit hits and read failures are disclosed. It keeps an
aggregate and one provisional sweep rather than an unbounded sample history.
Platform process enumeration still scales with host
process count and a single OS call cannot have a portable hard deadline; the
worker never runs on the watchdog thread. Stopping publishes one timestamp under
a lock before sending a wakeup; repeated stop calls return that same timestamp.
Sweep acceptance reads the published boundary, so delayed wake delivery cannot
admit later counters. Sweeps that finish at or after the stop or deadline are
discarded, but their collection cost remains recorded. Collector
startup and resource contention can perturb wall time and are not subtracted.

`capabilities` describes counter support separately from observed values. Linux
and macOS I/O measures storage activity, which can omit cached logical reads.
Windows reports transfer bytes, including file, device and pipe I/O; these are
not interchangeable metrics. CPU values are quantized to milliseconds before
conversion to nanoseconds. Underlying kernel counter resolution is unavailable.

The Linux I/O distinction follows the kernel's
[per-thread and process accounting paths](https://github.com/torvalds/linux/blob/v6.15/fs/proc/base.c#L2850)
and its [wait accounting](https://github.com/torvalds/linux/blob/v6.15/kernel/exit.c#L1169).

The initial identity read is included in collection cost. Cleanup includes
joining the worker; no collector survives the operation. No final cleanup or
grace sample is added to the phase's measurements.

GPU and Linux kernel high-water metrics have explicit unavailable capabilities
and a namespaced extension map for future providers. Such providers must specify
their own scope and precision rather than relabeling the sampled RSS peak.

## Verification

Deterministic tests cover tree aggregation, reparenting, disappearing processes,
counter regressions, PID reuse, unsupported providers, tracking caps and the
sampling budget. Linux tests reject a stale start time for a live PID, preserve
previous counters after rejection, and accept a matching sysinfo identity.
Worker tests cover lost diagnostics after panic, delayed stop notifications and
samples on both sides of a stop or deadline. Watchdog tests cover pending sampler
startup, non-reaping exit detection, signals, cancellation and unfinished pipes.
Subprocess fixtures exercise child CPU, memory and I/O, plus
retained resource evidence on success, failure, timeout and cancellation. Local
platform results are distinguished from the other platforms' CI coverage.
