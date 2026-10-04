# ADR 0006: Cooperative codec work bounds

Status: accepted

## Context

Content byte, expansion, coding-depth, and decoder-window limits bound retained
memory and representation growth, but they do not by themselves keep a large
codec operation from monopolizing an async executor thread. Surrounding hook
and network deadlines also cannot interrupt synchronous work performed inside
one codec poll.

The content layer needs finite scheduling and time policy without moving codec
objects into transport adapters, starting detached work that can outlive an
exchange, or claiming instruction-level preemption that the selected codec
libraries cannot provide.

## Decision

`ContentLimits` owns a `ContentWorkLimits` value with three finite settings:

- maximum input or output byte progress between cooperative executor yields;
- maximum wall-clock duration of one data-frame or completion operation; and
- maximum cumulative active wall-clock duration of one codec layer for a body.

The cumulative duration includes codec calls and cooperative yields made while
an operation is active. It excludes time between body frames, so a legitimately
slow network peer does not consume codec work time. The body duration must be
at least the operation duration. Defaults are a 64 KiB work quantum, two
seconds per operation, and thirty seconds per codec layer and body.

Decoding is asynchronous. Its private input reader exposes at most one work
quantum to a decoder poll, output is collected in pieces no larger than the
smaller of 8 KiB and that quantum, and the task yields when the configured
amount of input or output progress has accrued.
Encoding divides input into quantum-sized writes and applies the same yield and
deadline checks. The neutral and raw pass-through paths do not construct codecs
and retain their existing zero-codec fast path.

An operation or cumulative deadline violation returns the rustymiddle-owned
`ContentCodecError::Timeout` and makes that codec terminal. Dropping the owning
body task drops the in-progress codec future; no codec thread or blocking job
continues in the background. An embedding application that directly cancels a
borrowed codec operation must discard that codec object rather than attempting
to resume partially advanced compression state.

This is cooperative preemption at bounded progress boundaries. One call into a
dependency may still perform work before returning to rustymiddle. The input
quantum and fixed output buffer bound the data offered at that boundary, but
they are not an instruction counter and cannot forcibly interrupt native zstd
or another codec halfway through a call. Applications accepting hostile input
must keep byte, ratio, window, and work limits conservative together.

## Consequences

- Large codec operations give unrelated futures opportunities to run.
- Codec timeouts are typed, terminal, configurable, and independent of
  transport-specific idle deadlines.
- Per-body work accounting does not penalize network idle time.
- Decoder calls become async, which is an accepted pre-1.0 API change.
- Deadline checks use monotonic elapsed time at codec/yield boundaries; they do
  not promise hard real-time scheduling or exact CPU accounting.
- Tests cover configuration validation, exact time boundaries, cumulative
  exhaustion, terminal timeout behavior, cooperative encode/decode scheduling,
  and cancellation of independently owned codec tasks.

## Alternatives considered

- Wrap synchronous codec calls in an async timeout. Rejected because the timer
  cannot run while the same executor thread is blocked inside the call.
- Move every codec call to `spawn_blocking`. Rejected because aborting the
  awaiting future does not stop an already running blocking job, allowing work
  to outlive a cancelled exchange and making concurrency accounting harder.
- Call codecs on dedicated killable threads or processes. Deferred to a higher
  isolation layer if threat models eventually require hard CPU containment;
  it is substantially heavier than a transport-neutral content library.
- Count only bytes and omit deadlines. Rejected because codecs and scheduler
  delays can consume very different time for the same number of bytes.
