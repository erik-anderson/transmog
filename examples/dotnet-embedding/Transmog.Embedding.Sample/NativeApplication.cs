using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using Microsoft.Win32.SafeHandles;

namespace Transmog.Embedding.Sample;

// Language and ownership adapter. Schemas and behavior come from transmog_app::Application.
internal sealed unsafe class NativeApplication : IDisposable
{
    private const string LibraryName = "transmog_sdk";
    private readonly ApplicationHandle handle;

    public NativeApplication(string? bodyCacheDirectory = null)
    {
        if (NativeMethods.AbiVersion() != 2)
            throw new InvalidOperationException("The native application SDK requires ABI revision 2.");
        if (bodyCacheDirectory is null)
            handle = NativeMethods.Create();
        else
        {
            byte[] path = Encoding.UTF8.GetBytes(bodyCacheDirectory);
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

    public JsonElement Status() => Read(NativeMethods.Status(handle));
    public JsonElement ProductState() => Read(NativeMethods.ProductState(handle));
    public JsonElement SaveProductState(string json) => Input(NativeMethods.SaveProductState, json);
    public JsonElement StartProxy(string json) => Input(NativeMethods.StartProxy, json);
    public JsonElement StopProxy() => Read(NativeMethods.StopProxy(handle));
    public JsonElement Shutdown() => Read(NativeMethods.Shutdown(handle));
    public JsonElement QuerySessions(string json) => Input(NativeMethods.QuerySessions, json);
    public JsonElement QueryTrafficView(string json) => Input(NativeMethods.QueryTrafficView, json);
    public JsonElement TrafficViewRows(string viewId, nuint offset, nuint limit)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(viewId);
        fixed (byte* input = bytes)
            return Read(NativeMethods.TrafficViewRows(handle, input, (nuint)bytes.Length, offset, limit));
    }
    public JsonElement ReleaseTrafficView(string viewId) => Input(NativeMethods.ReleaseTrafficView, viewId);
    public SessionUpdates SubscribeSessionUpdates(Action<JsonElement> callback) => new(this, callback);
    public JsonElement SessionDetail(string id) => Input(NativeMethods.SessionDetail, id);
    public JsonElement ReadBodyRange(string json) => Input(NativeMethods.ReadBodyRange, json);
    public void Dispose() => handle.Dispose();

    private delegate ResultHandle InputOperation(ApplicationHandle app, byte* input, nuint length);
    private JsonElement Input(InputOperation operation, string text)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        ResultHandle result;
        fixed (byte* input = bytes) result = operation(handle, input, (nuint)bytes.Length);
        return Read(result);
    }

    private static JsonElement Read(ResultHandle result)
    {
        using (result)
        {
            if (result.IsInvalid)
                throw new InvalidOperationException("The SDK could not allocate an operation result.");
            bool succeeded = NativeMethods.Succeeded(result) == 1;
            byte[] bytes = succeeded
                ? Copy(NativeMethods.Json(result), NativeMethods.JsonLength(result))
                : Copy(NativeMethods.Error(result), NativeMethods.ErrorLength(result));
            using JsonDocument document = JsonDocument.Parse(bytes);
            JsonElement value = document.RootElement;
            if (!succeeded)
                throw new NativeApplicationException(
                    value.GetProperty("category").GetString()!,
                    value.GetProperty("message").GetString()!,
                    value.GetProperty("retryable").GetBoolean());
            return value.Clone();
        }
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
        private sealed class CallbackState(Action<JsonElement> callback)
        {
            internal readonly Action<JsonElement> Callback = callback;
            internal Exception? Error;
        }
        private readonly SessionUpdatesHandle handle;
        private readonly CallbackState state;
        private static readonly NativeMethods.SessionUpdateCallback DispatchCallback = Dispatch;
        public Exception? CallbackError => Volatile.Read(ref state.Error);

        internal SessionUpdates(NativeApplication app, Action<JsonElement> callback)
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
                    _ = Read(NativeMethods.SessionUpdatesStatus(handle));
            }
            catch { handle.Dispose(); throw; }
        }

        private static void Dispatch(nint context, nint bytes, nuint length)
        {
            var state = (CallbackState)GCHandle.FromIntPtr(context).Target!;
            try
            {
                using JsonDocument document = JsonDocument.Parse(Copy(bytes, length));
                state.Callback(document.RootElement.Clone());
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
        internal delegate void SessionUpdateCallback(nint context, nint json, nuint length);
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
        internal static extern ResultHandle SaveProductState(ApplicationHandle app, byte* input, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_start_proxy")]
        internal static extern ResultHandle StartProxy(ApplicationHandle app, byte* input, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_stop_proxy")]
        internal static extern ResultHandle StopProxy(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_shutdown")]
        internal static extern ResultHandle Shutdown(ApplicationHandle app);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_query_sessions")]
        internal static extern ResultHandle QuerySessions(ApplicationHandle app, byte* input, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_application_query_traffic_view")]
        internal static extern ResultHandle QueryTrafficView(ApplicationHandle app, byte* input, nuint length);
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
        internal static extern ResultHandle ReadBodyRange(ApplicationHandle app, byte* input, nuint length);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_succeeded")]
        internal static extern int Succeeded(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_json")]
        internal static extern nint Json(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_json_length")]
        internal static extern nuint JsonLength(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_error_json")]
        internal static extern nint Error(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_error_json_length")]
        internal static extern nuint ErrorLength(ResultHandle result);
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_free")]
        internal static extern void FreeResult(nint result);
    }
}
internal sealed class NativeApplicationException(string category, string message, bool retryable)
    : InvalidOperationException(message)
{
    public string Category { get; } = category;
    public bool Retryable { get; } = retryable;
}
