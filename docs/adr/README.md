# Architecture decision records

These records capture accepted constraints that still shape the checked-in
system. Implementation status, test commands, and future work belong in the
topic guides and roadmap instead of the ADRs.

| ADR | Decision |
|---|---|
| [0001](0001-native-stack.md) | Use LLVM/Ninja on Windows and one BoringSSL family. |
| [0002](0002-interception-boundaries.md) | Keep interception typed, transport-neutral, bounded, and separate from observation. |
| [0003](0003-monorepo-layers-and-control-stability.md) | Preserve layer boundaries and stabilize control only after an explicit human decision. |
| [0004](0004-async-content-codecs.md) | Use private async streaming codec adapters with explicit enabled formats. |
| [0005](0005-content-pipeline-composition.md) | Compose representation-aware content processing around core body plans. |
| [0006](0006-cooperative-codec-work-bounds.md) | Bound codec scheduling and active work cooperatively. |
| [0007](0007-tauri-webui-delivery.md) | Deliver the Windows Tauri/WebUI shell through a hardened custom protocol. |
| [0008](0008-traffic-workspace-and-script-isolation.md) | Keep traffic state above core and run scripts in an OS sandbox. |

Create a new ADR only for a durable cross-cutting decision or to supersede an
existing decision. Do not add completed task plans, implementation journals, or
verification transcripts here.
