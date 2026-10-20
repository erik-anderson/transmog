# Build-time JavaScript compiler runtime

The MAPI port evaluates `Transpose.Compiler.Library` **26.10.6985**, source
revision `c514aa842eb65a0c26715c897e12fbf9aad93c7c`, with
`Transpose.BCL` **26.9.5840**, source revision
`c9fde312a9a781ac0d3dd00533a074fe410a3776`.
Both NuGet packages declare Apache-2.0. `LICENSE` is the upstream license from
the pinned compiler revision. Runtime copyright: Object.NET, Inc. 2008–2019;
Curiosity GmbH 2019–2026.

This is an extension authoring dependency, not a host language service. The
ordinary app build and installed extension must not need .NET or a compiler.
Generated code must be a closed static JavaScript module graph; the existing
V8 code-generation restrictions and network-free OS sandbox remain mandatory.
Runtime string compilation/dynamic-import facilities are outside the extension
contract and must be disabled before any generated runtime is qualified.
