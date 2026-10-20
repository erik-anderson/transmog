// Developer-only reference runner. None of this .NET program is shipped in tmext.
using System.Globalization;
using System.Reflection;
using System.Text.Json;
using BlockParser;
using Fiddler;
using MapiInspector;
using MAPIInspector.Parsers;

internal static class Oracle
{
    private static readonly JsonSerializerOptions Json = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };
    public static int Main(string[] args)
    {
        if (GC.GetGCMemoryInfo().TotalAvailableMemoryBytes > 256 * 1024 * 1024)
        {
            Console.Error.WriteLine("The reference oracle requires its configured 256 MiB GC heap limit");
            return 2;
        }
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
        CultureInfo.CurrentUICulture = CultureInfo.InvariantCulture;
        if (args.Length == 1 && args[0] == "--inventory")
        {
            Console.WriteLine(JsonSerializer.Serialize(typeof(Block).Assembly.GetTypes()
                .Where(type => type.IsSubclassOf(typeof(Block)) && !type.IsAbstract && !type.ContainsGenericParameters)
                .OrderBy(type => type.FullName, StringComparer.Ordinal)
                .Select(type => new { type = type.FullName, constructors = type.GetConstructors().Select(ctor => ctor.GetParameters().Select(parameter => parameter.ParameterType.Name).ToArray()).ToArray() }), Json));
            return 0;
        }
        string line;
        while ((line = Console.ReadLine()) != null)
        {
            try
            {
                var input = JsonSerializer.Deserialize<Case>(line, Json);
                Console.WriteLine(JsonSerializer.Serialize(Run(input), Json));
            }
            catch (Exception error)
            {
                Console.WriteLine(JsonSerializer.Serialize(new { failure = error.GetType().Name }, Json));
            }
        }
        return 0;
    }

    private static object Run(Case fixture)
    {
        Partial.ResetPartialParameters();
        Partial.ResetPartialContextInformation();
        foreach (var side in new[] { "Get", "Put", "PutExtend" })
        {
            foreach (var field in new[] { "Session", "ServerUrl", "ProcessName", "ClientInfo" })
                typeof(Partial).GetProperty("Partial" + side + field).SetValue(null, null);
            typeof(Partial).GetProperty("IsOneMore" + side + "ByteToRead").SetValue(null, false);
            typeof(Partial).GetProperty("OneMore" + side + "Byte").SetValue(null, (byte)0);
        }
        MAPIParser.ResetHandleInformation();
        MAPIParser.TargetHandle.Clear();
        MAPIParser.ContextInformationCollection.Clear();
        MAPIParser.AllRopsList.Clear();
        MAPIParser.BuffersIsCompressed.Clear();
        MAPIParser.requestBytesForHexview.Clear();
        MAPIParser.responseBytesForHexview.Clear();
        MAPIParser.IsLooperCall = false;
        MAPIParser.NeedToParseCROPSLayer = false;
        MAPIParser.IsOnlyGetServerHandle = false;
        MAPIParser.InputPayLoadCompressedXOR = null;
        MAPIParser.OutputPayLoadCompressedXOR = null;
        MAPIParser.AuxPayLoadCompressedXOR = null;
        MAPIParser.BaseHeaders = null;
        _ = new DecodingContext();
        var sessions = (fixture.Context ?? Array.Empty<Message>()).Append(fixture.Message).Select((message, index) => Session(message, message.Order > 0 ? message.Order : index + 1)).ToArray();
        if (sessions.Length > 257 || sessions.Select(session => session.id).Distinct().Count() != sessions.Length)
            throw new ArgumentException("Invalid bounded fixture session ordering");
        SessionExtensions.AllSessionsNavigator.Init(sessions);
        var selected = sessions[^1];
        MAPIParser.ParsingSession = selected;
        Block result;
        if (fixture.Type != null)
        {
            if (fixture.PrepareRequest)
                MAPIParser.ParseRequestMessage(selected, out _);
            MAPIParser.ParsingSession = selected;
            var type = typeof(Block).Assembly.GetType(fixture.Type, true);
            result = Construct(type, fixture.Arguments ?? Array.Empty<JsonElement>());
            var parser = new BinaryParser(fixture.Message.Direction == "request" ? selected.requestBodyBytes : selected.responseBodyBytes) { Offset = fixture.Offset };
            if (fixture.Count > 0) parser.PushCap(fixture.Count);
            result.Parse(parser);
        }
        else
        {
            byte[] bytes;
            result = fixture.Message.Direction == "request" ? MAPIParser.ParseRequestMessage(selected, out bytes) : MAPIParser.ParseResponseMessage(selected, out bytes);
        }
        return Project(result);
    }

    private static Block Construct(Type type, JsonElement[] arguments)
    {
        foreach (var constructor in type.GetConstructors().Where(item => item.GetParameters().Length == arguments.Length))
        {
            try
            {
                var values = constructor.GetParameters().Select((parameter, index) =>
                {
                    var value = arguments[index];
                    if (value.ValueKind == JsonValueKind.Null && !parameter.ParameterType.IsValueType) return null;
                    if (parameter.ParameterType == typeof(bool)) return (object)value.GetBoolean();
                    if (parameter.ParameterType.IsEnum) return Enum.ToObject(parameter.ParameterType, value.GetInt64());
                    return Convert.ChangeType(value.GetInt64(), parameter.ParameterType, CultureInfo.InvariantCulture);
                }).ToArray();
                return (Block)constructor.Invoke(values);
            }
            catch (InvalidOperationException) { }
            catch (InvalidCastException) { }
        }
        throw new MissingMethodException(type.FullName, "fixture constructor");
    }

    private static Session Session(Message message, int id)
    {
        var session = new Session { id = id, LocalProcess = "bounded-conversation" };
        session.RequestHeaders["Content-Type"] = "application/mapi-http";
        session.ResponseHeaders["Content-Type"] = "application/mapi-http";
        session.RequestHeaders["X-RequestType"] = message.RequestType;
        session.RequestHeaders["X-ClientInfo"] = "synthetic-client";
        session.RequestHeaders.RequestPath = "/mapi/emsmdb";
        session.ResponseHeaders["X-ResponseCode"] = "0";
        session.requestBodyBytes = Convert.FromBase64String(message.Request ?? "");
        session.responseBodyBytes = Convert.FromBase64String(message.Response ?? "");
        return session;
    }

    private static object Project(Block block)
    {
        if (block == null) return null;
        var type = block.GetType();
        var scalarBlock = block;
        for (int depth = 0; depth < 4 && scalarBlock != null && scalarBlock is not IBlockT; depth++)
        {
            var scalarOwner = scalarBlock.GetType();
            scalarBlock = (scalarOwner.GetField("Value") ?? scalarOwner.GetField("value"))?.GetValue(scalarBlock) as Block;
        }
        var scalar = scalarBlock is IBlockT && scalarBlock.Parsed ? scalarBlock.GetType().GetProperty("Data").GetValue(scalarBlock) : null;
        var scalarType = scalar?.GetType();
        string raw = scalarType?.IsEnum == true ? Convert.ChangeType(scalar, Enum.GetUnderlyingType(scalarType), CultureInfo.InvariantCulture).ToString()
            : scalar is IFormattable number ? number.ToString(null, CultureInfo.InvariantCulture) : scalar?.ToString();
        return new
        {
            type = type.IsGenericType ? type.GetGenericTypeDefinition().FullName : type.FullName,
            text = block.Text,
            parsed = block.Parsed,
            offset = block.Offset.ToString(CultureInfo.InvariantCulture),
            size = block.Size.ToString(CultureInfo.InvariantCulture),
            valueType = scalarType?.FullName,
            raw,
            children = block.Children.Select(Project).ToArray()
        };
    }

    internal sealed class Case
    {
        public string Type { get; set; }
        public JsonElement[] Arguments { get; set; }
        public int Offset { get; set; }
        public int Count { get; set; }
        public bool PrepareRequest { get; set; }
        public Message Message { get; set; }
        public Message[] Context { get; set; }
    }
    internal sealed class Message
    {
        public int Order { get; set; }
        public string Direction { get; set; }
        public string RequestType { get; set; }
        public string Request { get; set; }
        public string Response { get; set; }
    }
}
