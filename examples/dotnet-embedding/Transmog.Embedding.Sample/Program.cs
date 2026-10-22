using System.Diagnostics;
using System.Net;
using System.Security.Cryptography;
using Transmog.Sdk;
using Transmog.Sdk.Models;

namespace Transmog.Embedding.Sample;

internal static class Program
{
    public static async Task<int> Main(string[] args)
    {
        try
        {
            if (args.Length == 0 || args.Contains("--help"))
            {
                Console.WriteLine("Transmog application embedding example\n" +
                    "Usage: --url <http-url> --output <path> [--overwrite]\n" +
                    "Starts a loopback proxy, sends host HTTP traffic through it, inspects the exchange, and shuts down.");
                return args.Length == 0 ? 2 : 0;
            }
            var options = Options.Parse(args);
            if (!options.Overwrite && File.Exists(options.Output))
                throw new ArgumentException("Output file exists; pass --overwrite to replace it.");

            // This directory belongs only to this invocation. Native owners are
            // disposed before removing it; product buffer preferences choose storage.
            DirectoryInfo cache = Directory.CreateTempSubdirectory("transmog-embedding-");
            try { return await RunAsync(options, cache.FullName); }
            finally { cache.Delete(recursive: true); }
        }
        catch (Exception error) when (error is ArgumentException or IOException or InvalidOperationException
            or DllNotFoundException or EntryPointNotFoundException or HttpRequestException
            or OperationCanceledException)
        {
            Console.Error.WriteLine($"error: {error.Message}");
            return 1;
        }
    }

    private static async Task<int> RunAsync(Options options, string cacheDirectory)
    {
        using var app = new Application(cacheDirectory);
        // These are the app's existing ProductState/ProxyStartRequest models.
        // Change the same preferences the desktop changes; no SDK-specific proxy settings.
        ProductState state = app.ProductState();
        app.SaveProductState(state with
        {
            Preferences = state.Preferences with { DecryptHttps = false, ConfigureSystemProxy = false },
            Privacy = state.Privacy with { RetainResponseBodies = true, BufferLimit = new BufferLimit.Custom(64 * 1024 * 1024) }
        });
        ProxyStartRequest request = Application.DefaultProxyStartRequest() with
        {
            Listen = "127.0.0.1:0",
            Route = ProxyRoute.Http1
        };
        AppStatus started = app.StartProxy(request);
        string listener = started.Listener!;
        Console.WriteLine($"Proxy listening on {listener}");
        using var changed = new SemaphoreSlim(0, 1);
        int notifications = 0;
        using var updates = app.SubscribeSessionUpdates(hint =>
        {
            // SessionHint signals a refresh, including when its lagged flag
            // is set. Read authoritative rows rather than applying deltas.
            _ = hint.Lagged;
            Interlocked.Increment(ref notifications);
            if (changed.CurrentCount == 0) changed.Release();
        });
        using var cancellation = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        CancellationToken token = cancellation.Token;

        // The host sends traffic. The SDK exposes the application that observes it.
        using (var handler = new HttpClientHandler
        {
            Proxy = new WebProxy($"http://{listener}"),
            UseProxy = true,
            AllowAutoRedirect = false,
            AutomaticDecompression = DecompressionMethods.None
        })
        using (var client = new HttpClient(handler) { Timeout = TimeSpan.FromSeconds(15) })
        using (HttpResponseMessage response = await client.GetAsync(
            options.Url, HttpCompletionOption.ResponseHeadersRead, token))
        {
            // Drain/hash the client response concurrently so native capture
            // can advance while the host pulls captured byte pages.
            Task<(long Bytes, byte[] Hash)> host = HashHostBodyAsync(response, token);
            try
            {
                if (!await changed.WaitAsync(TimeSpan.FromSeconds(10), token))
                    throw new InvalidOperationException("No native session update notification arrived.");
                string id = await FindSessionAsync(app, options.Url, changed, token);
                await using var output = new FileStream(options.Output,
                    options.Overwrite ? FileMode.Create : FileMode.CreateNew,
                    FileAccess.Write, FileShare.Read, 64 * 1024, useAsync: true);
                using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
                long bytes = 0;
                int chunks = 0;
                await foreach (ReadOnlyMemory<byte> chunk in app.ReadBodyChunksAsync(
                    id, "client-response", cancellationToken: token))
                {
                    bytes = checked(bytes + chunk.Length);
                    if (bytes > 16 * 1024 * 1024)
                        throw new InvalidOperationException("Example response exceeds 16 MiB.");
                    hash.AppendData(chunk.Span);
                    await output.WriteAsync(chunk, token);
                    if (++chunks == 1)
                    {
                        await output.FlushAsync(token);
                        Console.WriteLine("Native body streaming started.");
                    }
                }
                var received = await host;
                if (received.Bytes != bytes || !CryptographicOperations.FixedTimeEquals(received.Hash, hash.GetHashAndReset()))
                    throw new IOException("Captured body bytes did not match the host response.");
                if (updates.CallbackError is not null)
                    throw new InvalidOperationException("The session update callback failed.", updates.CallbackError);
                Console.WriteLine($"HTTP {(int)response.StatusCode}; wrote {bytes} captured bytes to {options.Output}");
                Console.WriteLine($"Captured body: {bytes} bytes in {chunks} chunks; SHA-256 matches host response.");
                Console.WriteLine($"Session updates received: {Volatile.Read(ref notifications)}");
                SessionDetail detail = app.SessionDetail(id);
                Console.WriteLine($"Exchange {detail.Id}: {detail.TerminalState}; {detail.StoredBodies.Length} retained body boundaries.");
            }
            finally
            {
                cancellation.Cancel();
                try { _ = await host; }
                catch (Exception error) when (error is IOException or HttpRequestException or OperationCanceledException) { }
            }
        }
        _ = app.StopProxy();
        app.Shutdown();
        return 0;
    }

    private static async Task<string> FindSessionAsync(
        Application app, Uri url, SemaphoreSlim changed, CancellationToken token)
    {
        // Use the same ordering snapshot and bounded row reads as the desktop.
        SessionQueryInput query = Application.DefaultSessionQueryInput() with
        {
            Method = "GET",
            Host = url.Host,
            Sort = new SessionSort(TrafficColumn.StartedAt, SortDirection.Descending)
        };
        var wait = Stopwatch.StartNew();
        string? id = null;
        while (wait.Elapsed < TimeSpan.FromSeconds(10))
        {
            token.ThrowIfCancellationRequested();
            TrafficView view = await Task.Run(() => app.QueryTrafficView(query), token);
            string viewId = view.Id;
            try
            {
                SessionSummary[] rows = await Task.Run(() => app.TrafficViewRows(viewId, 0, 100), token);
                foreach (SessionSummary session in rows)
                    if (session.Path == url.PathAndQuery)
                        id = session.Id;
            }
            finally { app.ReleaseTrafficView(viewId); }
            if (id is not null) break;
            // A periodic refresh also covers coalesced/lost refresh hints.
            _ = await changed.WaitAsync(TimeSpan.FromMilliseconds(100), token);
        }
        if (id is null) throw new InvalidOperationException("The captured exchange did not become available.");
        return id;
    }

    private static async Task<(long Bytes, byte[] Hash)> HashHostBodyAsync(
        HttpResponseMessage response, CancellationToken token)
    {
        await using Stream body = await response.Content.ReadAsStreamAsync(token);
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        byte[] chunk = new byte[64 * 1024];
        long bytes = 0;
        while (true)
        {
            int count = await body.ReadAsync(chunk, token);
            if (count == 0) break;
            bytes += count;
            if (bytes > 16 * 1024 * 1024)
                throw new InvalidOperationException("Example response exceeds 16 MiB.");
            hash.AppendData(chunk, 0, count);
        }
        return (bytes, hash.GetHashAndReset());
    }
}
internal sealed record Options(Uri Url, string Output, bool Overwrite)
{
    public static Options Parse(string[] args)
    {
        string? url = null, output = null;
        bool overwrite = false;
        for (int index = 0; index < args.Length; index++)
        {
            string argument = args[index];
            if (argument == "--overwrite") { overwrite = true; continue; }
            if (argument is not ("--url" or "--output"))
                throw new ArgumentException($"Unknown argument: {argument}");
            if (++index == args.Length || args[index].StartsWith("--"))
                throw new ArgumentException($"{argument} requires a value.");
            if (argument == "--url") url = args[index]; else output = args[index];
        }
        if (!Uri.TryCreate(url, UriKind.Absolute, out Uri? parsed) ||
            parsed.Scheme != Uri.UriSchemeHttp || string.IsNullOrEmpty(parsed.Host) ||
            !string.IsNullOrEmpty(parsed.UserInfo))
            throw new ArgumentException("--url must be an absolute HTTP URL without user information.");
        if (string.IsNullOrWhiteSpace(output)) throw new ArgumentException("--output is required.");
        return new Options(parsed, Path.GetFullPath(output), overwrite);
    }
}
