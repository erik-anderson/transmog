# Pinned SWC parser safety patch

This directory preserves the published `swc_ecma_parser` 27.0.7 crate, with a
narrow resource-safety patch. The archive and every local file are SHA-256
recorded in `transmog-snapshot.json`; `parser.patch` contains the complete diff
against the published archive. Do not edit the Cargo registry cache.

The patch adds a typed fatal `SyntaxError::ResourceLimit` and a monotonic
parser-state latch when token advancement reaches an 8 MiB native-stack reserve
or 16,384 token advances within one top-level item (including speculative
TypeScript parsing). A separate 262,144-advance budget bounds total module work.
Only a successfully parsed top-level item resets the partial-tree budget;
nested blocks and namespaces do not. This permits shallow bundles such as the
shipped MAPI visualizer without allowing larger partially constructed trees.
Cloned and speculative parser states share that latch. The Transmog wrapper
checks it even if upstream error recovery produces a module. Targets without a
supported native stack boundary fail closed.

Four iterative AST-wrapping loops are limited to 128 iterations: expression
subscripts (member/call/non-null chains), binary/assertion chains, TypeScript
qualified names and postfix array/index types. This bounds partial tree depth
on parser error paths that otherwise build deeply nested trees on constant
native stack. The shared token budget bounds partial AST construction when many
short loops compose across grammar levels; per-loop caps alone are insufficient.
Recursive grammar paths separately retain the stack reserve.

The first-party compiler also bounds source to 1 MiB, recursive AST families to
128 levels and 100,000 nodes, runs compiler work on guarded stack space, and
drains rejected ASTs iteratively. See `crates/script-compiler/src/safety.rs`.
Child-process probes cover simple and composite malformed deep trees; these
checks complement sanitizer fuzzing rather than claiming complete engine safety.
No JavaScript is executed by these compiler checks.

Upstream crate metadata declares Apache-2.0 but the published archive omits
license text. `LICENSE` is the upstream repository's Apache license retrieved
from `https://raw.githubusercontent.com/swc-project/swc/main/LICENSE`; its exact
bytes and URL are recorded in the receipt. The archive's `.cargo_vcs_info.json`
records the package's upstream source commit.
