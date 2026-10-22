using System.Globalization;
using System.Runtime.CompilerServices;
using System.Text.Json;

namespace Transmog.Embedding.Sample;

// A language adapter over the same bounded byte pages used by the desktop.
// No independent body storage, retention policy, or native stream state.
internal static class BodyReader
{
    public static async IAsyncEnumerable<ReadOnlyMemory<byte>> ReadBodyChunksAsync(
        this NativeApplication app, string sessionId, string boundary, int chunkSize = 64 * 1024,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        if (chunkSize is < 1 or > 16 * 1024 * 1024)
            throw new ArgumentOutOfRangeException(nameof(chunkSize));
        cancellationToken.ThrowIfCancellationRequested();
        using var changed = new SemaphoreSlim(0, 1);
        using var updates = app.SubscribeSessionUpdates(_ =>
        {
            if (changed.CurrentCount == 0) changed.Release();
        });
        ulong offset = 0;
        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();
            JsonElement detail = await Task.Run(() => app.SessionDetail(sessionId), cancellationToken);
            cancellationToken.ThrowIfCancellationRequested();
            JsonElement? metadata = null;
            foreach (JsonElement body in detail.GetProperty("storedBodies").EnumerateArray())
                if (body.GetProperty("boundary").GetString() == boundary) { metadata = body; break; }
            if (metadata is JsonElement retained)
            {
                string? availability = retained.GetProperty("availability").GetString();
                RequireReadable(availability);
                ulong length = retained.GetProperty("retainedBytes").GetUInt64();
                if (offset > length)
                    throw new IOException("The retained body changed while it was being read.");
                if (offset < length)
                {
                    string request = JsonSerializer.Serialize(new
                    {
                        sessionId,
                        boundary,
                        offset = offset.ToString(CultureInfo.InvariantCulture),
                        maxBytes = chunkSize
                    });
                    JsonElement page = await Task.Run(() => app.ReadBodyRange(request), cancellationToken);
                    cancellationToken.ThrowIfCancellationRequested();
                    RequireReadable(page.GetProperty("metadata").GetProperty("availability").GetString());
                    if (page.GetProperty("offset").GetString() != offset.ToString(CultureInfo.InvariantCulture))
                        throw new IOException("The body page returned a different offset.");
                    byte[] bytes = Convert.FromBase64String(page.GetProperty("bytesBase64").GetString()!);
                    if (bytes.Length == 0 || bytes.Length > chunkSize)
                        throw new IOException("The body page returned an invalid byte count.");
                    offset = checked(offset + (ulong)bytes.Length);
                    yield return bytes;
                    continue;
                }
                if (availability == "complete") yield break;
            }
            if (detail.GetProperty("terminalState").GetString() != "active")
                throw new IOException("The exchange ended without a complete retained body.");
            if (updates.CallbackError is not null)
                throw new IOException("Body refresh notifications failed.", updates.CallbackError);
            // A capturing prefix is not EOF. Hints are lossy, so also refresh
            // periodically while waiting for bytes or the final availability.
            _ = await changed.WaitAsync(TimeSpan.FromMilliseconds(100), cancellationToken);
        }
    }

    private static void RequireReadable(string? availability)
    {
        if (availability is not ("capturing" or "complete"))
            throw new IOException($"The retained body is {availability ?? "unavailable"}; a complete read is unavailable.");
    }
}
