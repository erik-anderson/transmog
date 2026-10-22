using System.Text.Json;
using Transmog.Embedding.Sample;

// Deterministic responses exercise the real iterator without networking or sleeps.
await CancellationAtMetadataCompletionAsync();
await CancellationAtPageCompletionAsync();
await LivePrefixAndDisposalAsync();
await RetentionFailureAsync();
Console.WriteLine("Managed body reader passed: cancellation at native-call completion, live prefix waiting, owned chunks, early disposal, and incomplete retention.");

static JsonElement Detail(string availability, int length) => JsonSerializer.SerializeToElement(new
{
    terminalState = "active",
    storedBodies = new[] { new { boundary = "client-response", availability, retainedBytes = length } }
});

static BodyPage Page(BodyAvailability availability, ulong offset, byte value) =>
    new(offset, offset + 1, 65536, [value], new BodyMetadata(true, "session", "client-response",
        offset + 1, offset + 1, offset + 1, availability, null, null, [], null, null));

static void Require(bool value, string message)
{
    if (!value) throw new InvalidOperationException(message);
}

static async Task ExpectCancellationAsync(Task<bool> read)
{
    try { _ = await read; }
    catch (OperationCanceledException) { return; }
    throw new InvalidOperationException("Cancellation at native-call completion was ignored.");
}

static async Task CancellationAtMetadataCompletionAsync()
{
    using var cancel = new CancellationTokenSource();
    var app = new NativeApplication
    {
        OnDetail = () => { cancel.Cancel(); return Detail("complete", 0); }
    };
    await using var chunks = app.ReadBodyChunksAsync("session", "client-response", cancellationToken: cancel.Token)
        .GetAsyncEnumerator();
    await ExpectCancellationAsync(chunks.MoveNextAsync().AsTask());
    Require(app.Subscriptions == 0, "Cancellation leaked a subscription.");
}

static async Task CancellationAtPageCompletionAsync()
{
    using var cancel = new CancellationTokenSource();
    var app = new NativeApplication
    {
        OnDetail = () => Detail("complete", 1),
        OnPage = (_, _, _, _) => { cancel.Cancel(); return Page(BodyAvailability.Complete, 0, 42); }
    };
    await using var chunks = app.ReadBodyChunksAsync("session", "client-response", cancellationToken: cancel.Token)
        .GetAsyncEnumerator();
    await ExpectCancellationAsync(chunks.MoveNextAsync().AsTask());
    Require(app.Subscriptions == 0, "Canceled page delivery leaked a subscription.");
}

static async Task LivePrefixAndDisposalAsync()
{
    using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(5));
    var prefixChecked = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
    int length = 1;
    var app = new NativeApplication
    {
        OnDetail = () =>
        {
            int retained = Volatile.Read(ref length);
            if (retained == 1) prefixChecked.TrySetResult();
            return Detail("capturing", retained);
        },
        OnPage = (session, boundary, offset, maxBytes) =>
        {
            Require(session == "session", "Session ID changed.");
            Require(boundary == "client-response", "Boundary changed.");
            Require(maxBytes == 1, "Page budget changed.");
            return Page(BodyAvailability.Capturing, offset, offset == 0 ? (byte)0 : (byte)255);
        }
    };
    var chunks = app.ReadBodyChunksAsync("session", "client-response", chunkSize: 1, cancellationToken: deadline.Token)
        .GetAsyncEnumerator();
    try
    {
        Require(await chunks.MoveNextAsync(), "Missing prefix chunk.");
        ReadOnlyMemory<byte> first = chunks.Current;
        Require(first.Span[0] == 0, "Prefix changed.");
        // The next detail call observes a capturing body at the current offset.
        prefixChecked = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        Task<bool> next = chunks.MoveNextAsync().AsTask();
        await prefixChecked.Task.WaitAsync(deadline.Token);
        Require(!next.IsCompleted, "A capturing prefix was treated as EOF.");
        Volatile.Write(ref length, 2);
        app.Notify();
        Require(await next, "No chunk after the body grew.");
        Require(first.Span[0] == 0 && chunks.Current.Span[0] == 255, "Yielded chunks did not retain their own bytes.");
    }
    finally { await chunks.DisposeAsync(); }
    Require(app.Subscriptions == 0, "Stopping enumeration early leaked a subscription.");
}

static async Task RetentionFailureAsync()
{
    foreach (BodyAvailability availability in new[] { BodyAvailability.Disabled, BodyAvailability.Truncated,
        BodyAvailability.Lost, BodyAvailability.QuotaOmitted, BodyAvailability.Evicted })
    {
        var app = new NativeApplication
        {
            OnDetail = () => Detail("capturing", 1),
            OnPage = (_, _, _, _) => Page(availability, 0, 42)
        };
        await using var chunks = app.ReadBodyChunksAsync("session", "client-response").GetAsyncEnumerator();
        bool failed = false;
        try { _ = await chunks.MoveNextAsync(); }
        catch (IOException) { failed = true; }
        Require(failed && app.Subscriptions == 0, $"{availability} must fail before yielding incomplete bytes.");
    }
}

namespace Transmog.Embedding.Sample
{
    // Minimal synchronous boundary fixture; the shared Application models and
    // actual native ownership are tested by test-sdk.py and the C++ contract.
    internal sealed class NativeApplication
    {
        internal Func<JsonElement> OnDetail = () => throw new InvalidOperationException("Unexpected detail read.");
        internal Func<string, string, ulong, nuint, BodyPage> OnPage = (_, _, _, _) => throw new InvalidOperationException("Unexpected page read.");
        private Action<JsonElement>? callback;
        internal int Subscriptions;
        internal JsonElement SessionDetail(string _) => OnDetail();
        internal BodyPage ReadBodyRange(string session, string boundary, ulong offset, nuint maxBytes) => OnPage(session, boundary, offset, maxBytes);
        internal SessionUpdates SubscribeSessionUpdates(Action<JsonElement> action)
        {
            callback = action;
            Subscriptions++;
            return new SessionUpdates(this);
        }
        internal void Notify() => callback?.Invoke(default);
        internal sealed class SessionUpdates(NativeApplication app) : IDisposable
        {
            internal Exception? CallbackError => null;
            public void Dispose() { app.callback = null; app.Subscriptions--; }
        }
    }
}
