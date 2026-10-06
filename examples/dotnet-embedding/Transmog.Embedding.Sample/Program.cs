using System.Text.Json;

namespace Transmog.Embedding.Sample;

internal static class Program
{
    public static async Task<int> Main(string[] args)
    {
        try
        {
            if (args.Contains("--help", StringComparer.OrdinalIgnoreCase) || args.Length == 0)
            {
                PrintUsage();
                return args.Length == 0 ? 2 : 0;
            }

            Options options = Options.Parse(args);
            FileMode outputMode = options.Overwrite ? FileMode.Create : FileMode.CreateNew;
            if (!options.Overwrite && File.Exists(options.OutputPath))
            {
                throw new ArgumentException(
                    $"Output file already exists: {options.OutputPath}. Pass --overwrite to replace it.");
            }

            var nativeRequest = new
            {
                url = options.Url.AbsoluteUri,
                protocols = options.Protocols,
                maxResponseBytes = options.MaxResponseBytes,
                timeoutMilliseconds = checked(options.TimeoutSeconds * 1000)
            };
            NativeFetchResponse response = NativeTransmog.Fetch(nativeRequest);

            await using (var output = new FileStream(
                options.OutputPath,
                outputMode,
                FileAccess.Write,
                FileShare.None,
                bufferSize: 64 * 1024,
                useAsync: true))
            {
                await output.WriteAsync(response.Body);
            }

            using JsonDocument metadata = JsonDocument.Parse(response.MetadataJson);
            string protocol = metadata.RootElement.GetProperty("protocol").GetString() ?? "unknown";
            Console.WriteLine(
                $"HTTP {response.StatusCode} over {protocol}; wrote {response.Body.Length} bytes to {options.OutputPath}");
            Console.WriteLine(response.MetadataJson);
            return 0;
        }
        catch (Exception error) when (
            error is ArgumentException
                or IOException
                or InvalidOperationException
                or JsonException
                or DllNotFoundException
                or EntryPointNotFoundException)
        {
            Console.Error.WriteLine($"error: {error.Message}");
            return 1;
        }
    }

    private static void PrintUsage()
    {
        Console.WriteLine(
            """
            Transmog .NET 10 embedding sample

            Usage:
              dotnet run --no-build --project examples/dotnet-embedding/Transmog.Embedding.Sample -- \
                --url <http-or-https-url> --output <path> [options]

            Options:
              --protocols <list>         Ordered h1,h2,h3 attempts (default: h2,h1)
              --max-response-bytes <n>   Response limit (default: 16777216; max: 268435456)
              --timeout-seconds <n>      Per-attempt/head and body deadline (default: 30; max: 300)
              --overwrite                Replace an existing output file
              --help                     Show this help
            """);
    }
}

internal sealed record Options(
    Uri Url,
    string[] Protocols,
    string OutputPath,
    int MaxResponseBytes,
    int TimeoutSeconds,
    bool Overwrite)
{
    private const int DefaultMaxResponseBytes = 16 * 1024 * 1024;
    private const int MaximumResponseBytes = 256 * 1024 * 1024;
    private const int DefaultTimeoutSeconds = 30;
    private const int MaximumTimeoutSeconds = 300;

    public static Options Parse(string[] args)
    {
        string? urlText = null;
        string? outputPath = null;
        string protocolsText = "h2,h1";
        int maxResponseBytes = DefaultMaxResponseBytes;
        int timeoutSeconds = DefaultTimeoutSeconds;
        bool overwrite = false;

        for (int index = 0; index < args.Length; index++)
        {
            string argument = args[index];
            switch (argument)
            {
                case "--url":
                    urlText = NextValue(args, ref index, argument);
                    break;
                case "--output":
                    outputPath = NextValue(args, ref index, argument);
                    break;
                case "--protocols":
                    protocolsText = NextValue(args, ref index, argument);
                    break;
                case "--max-response-bytes":
                    maxResponseBytes = ParseBoundedPositiveInt(
                        NextValue(args, ref index, argument),
                        argument,
                        MaximumResponseBytes);
                    break;
                case "--timeout-seconds":
                    timeoutSeconds = ParseBoundedPositiveInt(
                        NextValue(args, ref index, argument),
                        argument,
                        MaximumTimeoutSeconds);
                    break;
                case "--overwrite":
                    overwrite = true;
                    break;
                default:
                    throw new ArgumentException($"Unknown argument: {argument}");
            }
        }

        if (!Uri.TryCreate(urlText, UriKind.Absolute, out Uri? url)
            || (url.Scheme != Uri.UriSchemeHttp && url.Scheme != Uri.UriSchemeHttps)
            || string.IsNullOrEmpty(url.Host)
            || !string.IsNullOrEmpty(url.UserInfo))
        {
            throw new ArgumentException("--url must be an absolute HTTP or HTTPS URL without user information.");
        }
        if (string.IsNullOrWhiteSpace(outputPath))
        {
            throw new ArgumentException("--output is required.");
        }

        string[] protocols = protocolsText
            .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Select(value => value.ToLowerInvariant())
            .ToArray();
        if (protocols.Length == 0
            || protocols.Any(value => value is not ("h1" or "h2" or "h3"))
            || protocols.Distinct(StringComparer.Ordinal).Count() != protocols.Length)
        {
            throw new ArgumentException(
                "--protocols must be a non-empty, duplicate-free comma-separated list of h1, h2, and h3.");
        }

        return new Options(
            url,
            protocols,
            Path.GetFullPath(outputPath),
            maxResponseBytes,
            timeoutSeconds,
            overwrite);
    }

    private static string NextValue(string[] args, ref int index, string option)
    {
        index++;
        if (index >= args.Length || args[index].StartsWith("--", StringComparison.Ordinal))
        {
            throw new ArgumentException($"{option} requires a value.");
        }
        return args[index];
    }

    private static int ParseBoundedPositiveInt(string value, string option, int maximum)
    {
        if (!int.TryParse(value, out int parsed) || parsed <= 0 || parsed > maximum)
        {
            throw new ArgumentException($"{option} must be between 1 and {maximum}.");
        }
        return parsed;
    }
}
