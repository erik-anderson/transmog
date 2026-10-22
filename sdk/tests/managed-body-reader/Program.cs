using Transmog.Sdk;
using Transmog.Sdk.Models;

// Deterministic responses exercise the real iterator without networking or sleeps.
TypedModelRoundtrip();
await CancellationAtMetadataCompletionAsync();
await CancellationAtPageCompletionAsync();
await LivePrefixAndDisposalAsync();
await RetentionFailureAsync();
Console.WriteLine("Managed body reader passed: cancellation at native-call completion, live prefix waiting, owned chunks, early disposal, and incomplete retention.");

static void TypedModelRoundtrip()
{
    ProductState state = ModelMarshal.ReadProductState(default);
    state = state with
    {
        Privacy = state.Privacy with { BufferLimit = new BufferLimit.Custom(ulong.MaxValue) },
        Preferences = state.Preferences with
        {
            ProxyPort = ushort.MaxValue,
            ClientCertificatePolicy = new ClientCertificatePolicy(ClientCertificateDefault.Prompt,
            [new ClientCertificateRule("unicode-雪\0id", true, "example.test", 443,
                new ClientCertificateAction.Automatic(new ClientCertificateSelector("", [new("CN", "雪")], [])))])
        }
    };
    ProductState copied;
    using (var arena = new InputArena())
        copied = ModelMarshal.ReadProductState(ModelMarshal.WriteProductState(state, arena));
    Require(copied.Privacy.BufferLimit is BufferLimit.Custom { Bytes: ulong.MaxValue }, "64-bit enum payload changed.");
    Require(copied.Preferences.ProxyPort == ushort.MaxValue, "Optional scalar changed.");
    var rule = copied.Preferences.ClientCertificatePolicy.Rules.Single();
    Require(rule.Id == "unicode-雪\0id" && rule.Enabled && rule.Port == 443, "Owned nested fields changed.");
    var selector = ((ClientCertificateAction.Automatic)rule.Mode).Selector;
    Require(selector.Identity == "" && selector.Subject.Single().Value == "雪" && selector.Issuer.Length == 0,
        "Maps, Unicode, or present empty text changed.");
    using var invalid = new InputArena();
    bool rejected = false;
    try { invalid.Text("\ud800"); }
    catch (System.Text.EncoderFallbackException) { rejected = true; }
    Require(rejected, "Invalid UTF-16 must not be silently replaced.");
}

static SessionDetail Detail(string availability, int length) =>
    ModelMarshal.ReadSessionDetail(default) with
    {
        TerminalState = "active",
        StoredBodies = [new StoredBodyMetadata(true, "session", "client-response", (ulong)length,
            (ulong)length, (ulong)length, Enum.Parse<BodyAvailability>(availability, ignoreCase: true),
            null, null, [], null, null)]
    };

static BodyPage Page(BodyAvailability availability, ulong offset, byte value) =>
    new(offset, offset + 1, 65536, [value], new StoredBodyMetadata(true, "session", "client-response",
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
    var app = new Application
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
    var app = new Application
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
    var app = new Application
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
        var app = new Application
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

namespace Transmog.Sdk
{
    // Minimal synchronous boundary fixture; the shared Application models and
    // actual native ownership are tested by test-sdk.py and the C++ contract.
    public sealed class Application
    {
        internal Func<SessionDetail> OnDetail = () => throw new InvalidOperationException("Unexpected detail read.");
        internal Func<string, string, ulong, nuint, BodyPage> OnPage = (_, _, _, _) => throw new InvalidOperationException("Unexpected page read.");
        private Action<SessionHint>? callback;
        internal int Subscriptions;
        internal SessionDetail SessionDetail(string _) => OnDetail();
        internal BodyPage ReadBodyRange(string session, string boundary, ulong offset, nuint maxBytes) => OnPage(session, boundary, offset, maxBytes);
        internal SessionUpdates SubscribeSessionUpdates(Action<SessionHint> action)
        {
            callback = action;
            Subscriptions++;
            return new SessionUpdates(this);
        }
        internal void Notify() => callback?.Invoke(new SessionHint(null, 1, false));
        internal sealed class SessionUpdates(Application app) : IDisposable
        {
            internal Exception? CallbackError => null;
            public void Dispose() { app.callback = null; app.Subscriptions--; }
        }
    }
}
