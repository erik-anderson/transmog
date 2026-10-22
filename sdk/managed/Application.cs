using System.Runtime.InteropServices;
using System.Text;
using Transmog.Sdk.Models;
using Microsoft.Win32.SafeHandles;

namespace Transmog.Sdk;

// Language and ownership adapter. Schemas and behavior come from transmog_app::Application.
public sealed unsafe class Application : IDisposable
{
    private const string LibraryName = "transmog_sdk";
    private readonly ApplicationHandle handle;

    public Application(string? bodyCacheDirectory = null)
    {
        RequireAbiVersion();
        if (bodyCacheDirectory is null)
            handle = NativeMethods.Create();
        else
        {
            byte[] path = Utf8.GetBytes(bodyCacheDirectory);
            fixed (byte* input = path)
                handle = NativeMethods.CreateWithBodyStore(input, (nuint)path.Length);
        }
        try
        {
            if (handle.IsInvalid)
                throw new InvalidOperationException("The SDK could not allocate an application handle.");
            if (NativeMethods.Valid(handle) != 1)
                _ = Status(); // Returns the original application creation error.
        }
        catch { handle.Dispose(); throw; }
    }

    public AppStatus Status() => Read(NativeMethods.Status(handle), ModelMarshal.ReadAppStatusResult);
    public ProductState ProductState() => Read(NativeMethods.ProductState(handle), ModelMarshal.ReadProductStateResult);
    public static ProxyStartRequest DefaultProxyStartRequest()
    {
        RequireAbiVersion();
        return Read(NativeMethods.DefaultProxyStartRequest(), ModelMarshal.ReadProxyStartRequestResult);
    }
    public static SessionQueryInput DefaultSessionQueryInput()
    {
        RequireAbiVersion();
        return Read(NativeMethods.DefaultSessionQueryInput(), ModelMarshal.ReadSessionQueryInputResult);
    }
    public ProductState SaveProductState(ProductState value)
    {
        using var arena = new InputArena();
        var input = ModelMarshal.WriteProductState(value, arena);
        return Read(NativeMethods.SaveProductState(handle, in input), ModelMarshal.ReadProductStateResult);
    }
    public AppStatus StartProxy(ProxyStartRequest value)
    {
        using var arena = new InputArena();
        var input = ModelMarshal.WriteProxyStartRequest(value, arena);
        return Read(NativeMethods.StartProxy(handle, in input), ModelMarshal.ReadAppStatusResult);
    }
    public AppStatus StopProxy() => Read(NativeMethods.StopProxy(handle), ModelMarshal.ReadAppStatusResult);
    public void Shutdown() => Check(NativeMethods.Shutdown(handle));
    public SessionPage QuerySessions(SessionQueryInput value)
    {
        using var arena = new InputArena();
        var input = ModelMarshal.WriteSessionQueryInput(value, arena);
        return Read(NativeMethods.QuerySessions(handle, in input), ModelMarshal.ReadSessionPageResult);
    }
    public TrafficView QueryTrafficView(SessionQueryInput value)
    {
        using var arena = new InputArena();
        var input = ModelMarshal.WriteSessionQueryInput(value, arena);
        return Read(NativeMethods.QueryTrafficView(handle, in input), ModelMarshal.ReadTrafficViewResult);
    }
    public SessionSummary[] TrafficViewRows(string viewId, nuint offset, nuint limit)
    {
        byte[] bytes = Utf8.GetBytes(viewId);
        fixed (byte* input = bytes)
            return Read(NativeMethods.TrafficViewRows(handle, input, (nuint)bytes.Length, offset, limit), ModelMarshal.ReadSessionRowsResult);
    }
    public void ReleaseTrafficView(string viewId) => Check(Input(NativeMethods.ReleaseTrafficView, viewId));
    public SessionUpdates SubscribeSessionUpdates(Action<SessionHint> callback) => new(this, callback);
    public SessionDetail SessionDetail(string id) => Read(Input(NativeMethods.SessionDetail, id), ModelMarshal.ReadSessionDetailResult);
    public BodyPage ReadBodyRange(string sessionId, string boundary, ulong offset, nuint maxBytes = 64 * 1024)
    {
        byte[] session = Utf8.GetBytes(sessionId), stage = Utf8.GetBytes(boundary);
        BodyPageHandle page;
        fixed (byte* sessionInput = session)
        fixed (byte* boundaryInput = stage)
            page = NativeMethods.ReadBodyRange(handle, sessionInput, (nuint)session.Length,
                boundaryInput, (nuint)stage.Length, offset, maxBytes);
        using (page)
        {
            if (page.IsInvalid) throw new InvalidOperationException("The SDK could not allocate a body page.");
            if (NativeMethods.BodyPageSucceeded(page) != 1)
            {
                NativeError error = NativeMethods.BodyPageError(page);
                throw new TransmogException((ErrorCategory)error.Category, Text(error.Message)!, error.Retryable != 0);
            }
            NativeBodyPage value = NativeMethods.BodyPageView(page);
            if (value.ContentCodingCount > int.MaxValue || value.Bytes.Size > maxBytes ||
                value.Bytes.Size > value.ReadLimit || value.Offset != offset ||
                value.Offset > value.TotalBytes || value.Bytes.Size > value.TotalBytes - value.Offset ||
                value.RetainedBytes != value.TotalBytes)
                throw new InvalidOperationException("The SDK returned an invalid body page.");
            string[] codings = new string[(int)value.ContentCodingCount];
            for (int index = 0; index < codings.Length; index++)
                codings[index] = Text(NativeMethods.BodyPageContentCoding(page, (nuint)index))!;
            return new BodyPage(value.Offset, value.TotalBytes, value.ReadLimit,
                Copy(value.Bytes.Data, value.Bytes.Size), new StoredBodyMetadata(value.LengthKnown != 0,
                    Text(value.ExchangeId)!, Text(value.Boundary)!, value.ObservedBytes,
                    value.HasWireBodyBytes != 0 ? value.WireBodyBytes : null, value.RetainedBytes,
                    value.Availability, Text(value.MediaType), Text(value.Charset), codings,
                    Text(value.Sha256), Text(value.Reason)));
        }
    }
    public void Dispose() => handle.Dispose();

    private static readonly UTF8Encoding Utf8 = new(false, true);
    private static void RequireAbiVersion()
    {
        if (NativeMethods.AbiVersion() != 4)
            throw new InvalidOperationException("The native application SDK requires ABI revision 4.");
    }
    private delegate ResultHandle InputOperation(ApplicationHandle app, byte* input, nuint length);
    private ResultHandle Input(InputOperation operation, string text)
    {
        byte[] bytes = Utf8.GetBytes(text);
        fixed (byte* input = bytes) return operation(handle, input, (nuint)bytes.Length);
    }

    private static void RequireSuccess(ResultHandle result)
    {
        if (result.IsInvalid) throw new InvalidOperationException("The SDK could not allocate an operation result.");
        if (NativeMethods.Succeeded(result) == 1) return;
        NativeError error = NativeMethods.Error(result);
        throw new TransmogException((ErrorCategory)error.Category, Text(error.Message)!, error.Retryable != 0);
    }
    private static T Read<T>(ResultHandle result, Func<nint, T> read)
    {
        using (result)
        {
            RequireSuccess(result);
            T value = read(result.DangerousGetHandle());
            GC.KeepAlive(result);
            return value;
        }
    }
    private static void Check(ResultHandle result)
    {
        using (result) RequireSuccess(result);
    }

    private static byte[] Copy(nint pointer, nuint length)
    {
        if (length == 0) return [];
        if (pointer == 0 || length > int.MaxValue)
            throw new InvalidOperationException("The SDK returned an invalid buffer.");
        byte[] bytes = new byte[(int)length];
        Marshal.Copy(pointer, bytes, 0, bytes.Length);
        return bytes;
    }

    private static string? Text(NativeBytes value) => value.Data == 0 ? null
        : Utf8.GetString(Copy(value.Data, value.Size));

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeBytes { internal nint Data; internal nuint Size; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeError { internal uint Category; internal NativeBytes Message; internal uint Retryable; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeBodyPage
    {
        internal NativeBytes Bytes;
        internal ulong Offset, TotalBytes;
        internal nuint ReadLimit;
        internal BodyAvailability Availability;
        internal uint LengthKnown;
        internal ulong ObservedBytes, RetainedBytes;
        internal uint HasWireBodyBytes;
        internal ulong WireBodyBytes;
        internal NativeBytes ExchangeId, Boundary, MediaType, Charset, Sha256, Reason;
        internal nuint ContentCodingCount;
    }

    private sealed class ApplicationHandle : SafeHandleZeroOrMinusOneIsInvalid
    {
        public ApplicationHandle() : base(true) { }
        protected override bool ReleaseHandle() { NativeMethods.FreeApplication(handle); return true; }
    }
    private sealed class ResultHandle : SafeHandleZeroOrMinusOneIsInvalid
    {
        public ResultHandle() : base(true) { }
        protected override bool ReleaseHandle() { NativeMethods.FreeResult(handle); return true; }
    }
    private sealed class BodyPageHandle : SafeHandleZeroOrMinusOneIsInvalid
    {
        public BodyPageHandle() : base(true) { }
        protected override bool ReleaseHandle() { NativeMethods.FreeBodyPage(handle); return true; }
    }
    private sealed class SessionUpdatesHandle : SafeHandleZeroOrMinusOneIsInvalid
    {
        internal GCHandle CallbackContext;
        public SessionUpdatesHandle() : base(true) { }
        protected override bool ReleaseHandle()
        {
            NativeMethods.FreeSessionUpdates(handle);
            if (CallbackContext.IsAllocated) CallbackContext.Free();
            return true;
        }
    }

    // Owns both the native subscription and the managed callback root. Native
    // release waits for delivery before releasing the root (including on finalization).
    public sealed class SessionUpdates : IDisposable
    {
        private sealed class CallbackState(Action<SessionHint> callback)
        {
            internal readonly Action<SessionHint> Callback = callback;
            internal Exception? Error;
        }
        private readonly SessionUpdatesHandle handle;
        private readonly CallbackState state;
        private static readonly NativeMethods.SessionUpdateCallback DispatchCallback = Dispatch;
        public Exception? CallbackError => Volatile.Read(ref state.Error);

        internal SessionUpdates(Application app, Action<SessionHint> callback)
        {
            ArgumentNullException.ThrowIfNull(callback);
            state = new CallbackState(callback);
            GCHandle context = GCHandle.Alloc(state);
            try
            {
                handle = NativeMethods.SubscribeSessionUpdates(app.handle, DispatchCallback, GCHandle.ToIntPtr(context));
            }
            catch { context.Free(); throw; }
            if (handle.IsInvalid)
            {
                context.Free();
                handle.Dispose();
                throw new InvalidOperationException("The SDK could not allocate a session subscription.");
            }
            handle.CallbackContext = context;
            try
            {
                if (NativeMethods.SessionUpdatesValid(handle) != 1)
                    Check(NativeMethods.SessionUpdatesStatus(handle));
            }
            catch { handle.Dispose(); throw; }
        }

        private static void Dispatch(nint context, nint hint)
        {
            var state = (CallbackState)GCHandle.FromIntPtr(context).Target!;
            try
            {
                state.Callback(ModelMarshal.ReadSessionHint(ModelMemory.Value<NativeSessionHint>(hint)));
            }
            catch (Exception error)
            {
                // Managed exceptions must never cross the native callback boundary.
                Interlocked.CompareExchange(ref state.Error, error, null);
            }
        }
        public void Dispose() => handle.Dispose();
    }
    private static class NativeMethods
    {
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        internal delegate void SessionUpdateCallback(nint context, nint hint);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_abi_version")]
        internal static extern uint AbiVersion();
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_create")]
        internal static extern ApplicationHandle Create();
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_create_with_body_store")]
        internal static extern ApplicationHandle CreateWithBodyStore(byte* path, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_valid")]
        internal static extern int Valid(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_free")]
        internal static extern void FreeApplication(nint app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_status")]
        internal static extern ResultHandle Status(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_product_state")]
        internal static extern ResultHandle ProductState(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_save_product_state")]
        internal static extern ResultHandle SaveProductState(ApplicationHandle app, in NativeProductState input);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_start_proxy")]
        internal static extern ResultHandle StartProxy(ApplicationHandle app, in NativeProxyStartRequest input);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_stop_proxy")]
        internal static extern ResultHandle StopProxy(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_shutdown")]
        internal static extern ResultHandle Shutdown(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_query_sessions")]
        internal static extern ResultHandle QuerySessions(ApplicationHandle app, in NativeSessionQueryInput input);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_query_traffic_view")]
        internal static extern ResultHandle QueryTrafficView(ApplicationHandle app, in NativeSessionQueryInput input);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_traffic_view_rows")]
        internal static extern ResultHandle TrafficViewRows(ApplicationHandle app, byte* id, nuint length, nuint offset, nuint limit);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_release_traffic_view")]
        internal static extern ResultHandle ReleaseTrafficView(ApplicationHandle app, byte* id, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_subscribe_session_updates")]
        internal static extern SessionUpdatesHandle SubscribeSessionUpdates(ApplicationHandle app, SessionUpdateCallback callback, nint context);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_session_updates_valid")]
        internal static extern int SessionUpdatesValid(SessionUpdatesHandle updates);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_session_updates_status")]
        internal static extern ResultHandle SessionUpdatesStatus(SessionUpdatesHandle updates);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_session_updates_free")]
        internal static extern void FreeSessionUpdates(nint updates);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_session_detail")]
        internal static extern ResultHandle SessionDetail(ApplicationHandle app, byte* input, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_read_body_range")]
        internal static extern BodyPageHandle ReadBodyRange(ApplicationHandle app, byte* session, nuint sessionLength,
            byte* boundary, nuint boundaryLength, ulong offset, nuint maxBytes);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_body_page_succeeded")]
        internal static extern int BodyPageSucceeded(BodyPageHandle page);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_body_page_error")]
        internal static extern NativeError BodyPageError(BodyPageHandle page);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_body_page_get_view")]
        internal static extern NativeBodyPage BodyPageView(BodyPageHandle page);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_body_page_content_coding")]
        internal static extern NativeBytes BodyPageContentCoding(BodyPageHandle page, nuint index);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_body_page_free")]
        internal static extern void FreeBodyPage(nint page);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_succeeded")]
        internal static extern int Succeeded(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_error")]
        internal static extern NativeError Error(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_default_proxy_start_request")]
        internal static extern ResultHandle DefaultProxyStartRequest();
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_default_session_query_input")]
        internal static extern ResultHandle DefaultSessionQueryInput();
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_free")]
        internal static extern void FreeResult(nint result);
    }
}
public sealed class TransmogException(ErrorCategory category, string message, bool retryable)
    : InvalidOperationException(message)
{
    public ErrorCategory Category { get; } = category;
    public bool Retryable { get; } = retryable;
}
