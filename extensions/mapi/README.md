# MAPI extension implementation and qualification

The bundled visualizer uses the pinned Office MAPI inspector as its behavioral
reference. This directory owns its authoring tools and synthetic parity corpus.
The protocol source snapshot and attribution live in
[`third_party/mapi-inspector`](../../third_party/mapi-inspector/README.md).

The developer-only .NET oracle preserves the original binary parser. Run:

```text
python scripts/test-mapi-reference.py
```

This verifies all 651 source hashes, the 529 concrete parser block types and
constructor shapes, nonzero little-endian transport fields, signed 64-bit values
beyond JavaScript's safe integer range, GUID byte order and exact byte ranges.
It is the reference-harness gate, not a claim of completed MAPI parity. No
sanitized Office capture is included or claimed to have been tested.

The production runtime remains JavaScript ES2022. A pinned build-time compiler
can preserve the upstream protocol implementation while emitting a closed static
JavaScript graph. Compiler, source language and reference-oracle dependencies
must not become installed runtime dependencies. Code generation, network calls,
Fiddler UI integration and global capture enumeration remain unavailable.

Before bundling, require differential coverage of:

| Area | Required synthetic cases |
| --- | --- |
| Mailbox transport | Connect, Disconnect, Execute and NotificationWait, both directions; nonzero flags/status/errors and auxiliary buffers |
| Address book | All dispatch operations in the pinned MAPI parser; NSPI structures, counts, strings and nullable fields |
| ROPs | Every request/response dispatcher branch; meaningful nonzero operations and handle tables, errors and unknown ROPs |
| Data | Property scalars, wide integers, GUIDs, flags, strings, multivalues, restrictions and nested entry IDs |
| Transformations | XOR, LZ77, combined and multi-buffer payloads; derived bytes with enclosing encoded provenance |
| FastTransfer | Representative lexical structures, partial continuations, synchronization and cross-exchange handles |
| Context and live | Bounded retained sessions only; missing/out-of-order context, same-origin grants, idempotent updates, cancellation and unload |
| Robustness | Truncation, corrupt counts/lengths, unsupported operations, bounded allocations/output/work and network denial in the real sandbox |

Known unsupported cases must be resolved before shipment. Full protocol-class
compilation or zero-filled structures alone cannot satisfy this matrix. Host
rendering and byte selection also need native packaged/unpacked qualification.
