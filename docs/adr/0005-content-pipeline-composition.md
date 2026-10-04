# ADR 0005: Content pipeline composition boundary

Status: accepted

## Context

Hooks v2 owns protocol-neutral body plans while `rustymiddle-content` owns HTTP
content codings. Core must express whether a plan expects raw coded bytes or
decoded representation bytes without depending on codec libraries. The content
layer must compose multiple codings, preserve exact pass-through behavior, and
fail before body commitment when independently registered hooks are
incompatible.

## Decision

Each request or response body action pairs its `BodyPlan` with one typed
`BodyRepresentation`: `Neutral`, `Raw`, `DecodedRequired`, or
`DecodedIfSupported`.

Core aggregates those requirements while constructing `BodyPipeline`:

- neutral pass-through does not activate content processing;
- required decoded dominates optional decoded;
- raw plans compose only with raw or neutral plans;
- any raw/decoded mixture is a typed planning error before a pump starts;
- optional decoded stages can be removed before processing when the coding is
  unsupported.

`rustymiddle-content::ContentBodyPipeline` wraps the resulting core pipeline.
It decodes the declared coding stack in reverse sender order, applies Hooks v2,
and re-encodes in forward sender order when policy preserves the original
coding. It owns whole-stack byte and expansion accounting in addition to each
codec layer's bounds. Trailers remain terminal and representation metadata is
repaired both before decoded hooks and after hook output.

An unchanged neutral pipeline retains the original headers, bytes, and frame
boundaries. A raw pipeline never invokes a codec and preserves
`Content-Encoding`, but invalidates length, validators, digests, and range
metadata when its body may change. A decoded replacement or discard can drain
source bytes without constructing decoders when no stage before it inspects the
source; output is still identity or explicitly re-encoded according to policy.

Codec objects and dependency errors remain private behind rustymiddle-owned
types. Runtime policy and transport wiring remain a higher-layer concern.

## Consequences

- Applications can request semantic content processing without importing
  Hyper, quiche, or codec implementation types.
- Independently registered raw and decoded extensions cannot silently reinterpret
  each other's bytes.
- Unsupported optional inspection is a byte-exact bypass; malformed input and
  required unsupported coding still fail closed.
- Recompression is never imposed on neutral traffic.
- The pre-1.0 Hooks v2 body-action constructors change to require an explicit
  representation choice for every body-changing plan.

## Alternatives considered

- Put codec variants in `rustymiddle-core`. Rejected because it reverses the
  intended dependency direction and makes core responsible for content policy.
- Infer decoded processing from `Content-Encoding` whenever any hook exists.
  Rejected because it destroys the raw fast path and makes raw tools ambiguous.
- Let registration order convert implicitly between raw and decoded stages.
  Rejected because a reordered extension chain would silently change byte
  interpretation and resource use.
- Decode before replacement or discard unconditionally. Rejected because
  discarded attacker-controlled content should not consume decompression CPU or
  fail a replacement that never observes it.
