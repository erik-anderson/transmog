using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Transmog.Embedding.Sample;

internal sealed record NativeFetchResponse(ushort StatusCode, byte[] Body, string MetadataJson);

internal static unsafe class NativeTransmog
{
    private const string LibraryName = "transmog_dotnet_sample_native";
    private const uint SupportedAbiVersion = 1;

    public static NativeFetchResponse Fetch(object request)
    {
        uint actualAbiVersion = NativeMethods.AbiVersion();
        if (actualAbiVersion != SupportedAbiVersion)
        {
            throw new InvalidOperationException(
                $"Native bridge ABI {actualAbiVersion} is incompatible with expected ABI {SupportedAbiVersion}.");
        }

        byte[] requestJson = JsonSerializer.SerializeToUtf8Bytes(request);
        nint handle;
        fixed (byte* requestPointer = requestJson)
        {
            handle = NativeMethods.Fetch(requestPointer, checked((nuint)requestJson.Length));
        }

        if (handle == 0)
        {
            throw new InvalidOperationException("The native bridge returned a null result handle.");
        }

        try
        {
            if (NativeMethods.FetchSucceeded(handle) != 1)
            {
                throw new InvalidOperationException(CopyUtf8(
                    NativeMethods.FetchError(handle),
                    NativeMethods.FetchErrorLength(handle)));
            }

            return new NativeFetchResponse(
                NativeMethods.FetchStatusCode(handle),
                CopyBytes(NativeMethods.FetchBody(handle), NativeMethods.FetchBodyLength(handle)),
                CopyUtf8(
                    NativeMethods.FetchMetadata(handle),
                    NativeMethods.FetchMetadataLength(handle)));
        }
        finally
        {
            NativeMethods.FetchFree(handle);
        }
    }

    private static string CopyUtf8(nint pointer, nuint length)
    {
        return Encoding.UTF8.GetString(CopyBytes(pointer, length));
    }

    private static byte[] CopyBytes(nint pointer, nuint length)
    {
        if (length == 0)
        {
            return [];
        }
        if (pointer == 0)
        {
            throw new InvalidOperationException("The native bridge returned a null buffer.");
        }
        if (length > int.MaxValue)
        {
            throw new InvalidOperationException("The native bridge returned an oversized buffer.");
        }

        byte[] copy = new byte[checked((int)length)];
        Marshal.Copy(pointer, copy, 0, copy.Length);
        return copy;
    }

    private static class NativeMethods
    {
        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_abi_version")]
        internal static extern uint AbiVersion();

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch")]
        internal static extern nint Fetch(byte* requestJson, nuint requestJsonLength);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_succeeded")]
        internal static extern int FetchSucceeded(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_status_code")]
        internal static extern ushort FetchStatusCode(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_body")]
        internal static extern nint FetchBody(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_body_length")]
        internal static extern nuint FetchBodyLength(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_metadata")]
        internal static extern nint FetchMetadata(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_metadata_length")]
        internal static extern nuint FetchMetadataLength(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_error")]
        internal static extern nint FetchError(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_error_length")]
        internal static extern nuint FetchErrorLength(nint handle);

        [DllImport(LibraryName, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true,
            EntryPoint = "transmog_dotnet_sample_fetch_free")]
        internal static extern void FetchFree(nint handle);
    }
}
