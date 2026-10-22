namespace Transmog.Embedding.Sample;

internal enum BodyAvailability : uint
{
    Unavailable = 0, Capturing = 1, Complete = 2, Truncated = 3,
    Lost = 4, Evicted = 5, Disabled = 6, QuotaOmitted = 7
}

// Managed copies of the shared Application page and metadata. Native views are
// copied before the owned page is freed; no borrowed native pointer escapes.
internal sealed record BodyMetadata(
    bool LengthKnown, string ExchangeId, string Boundary, ulong ObservedBytes,
    ulong? WireBodyBytes, ulong RetainedBytes, BodyAvailability Availability,
    string? MediaType, string? Charset, string[] ContentCodings, string? Sha256,
    string? Reason);

internal sealed record BodyPage(
    ulong Offset, ulong TotalBytes, nuint ReadLimit, byte[] Bytes, BodyMetadata Metadata);
