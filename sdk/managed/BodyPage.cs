using Transmog.Sdk.Models;

namespace Transmog.Sdk;

// Owned managed copies; no borrowed pointer escapes the native page owner.
public sealed record BodyPage(
    ulong Offset, ulong TotalBytes, nuint ReadLimit, byte[] Bytes, StoredBodyMetadata Metadata);
