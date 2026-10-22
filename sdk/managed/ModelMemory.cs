using System.Runtime.InteropServices;
using System.Text;

namespace Transmog.Sdk.Models;

[StructLayout(LayoutKind.Sequential)]
internal struct NativeText { internal nint Data; internal nuint Size; }
[StructLayout(LayoutKind.Sequential)]
internal struct NativeSlice { internal nint Data; internal nuint Size; }
[StructLayout(LayoutKind.Sequential)]
internal struct NativeTextEntry { internal NativeText Key; internal NativeText Value; }

internal static unsafe class ModelMemory
{
    private static readonly UTF8Encoding Utf8 = new(false, true);
    internal static T Value<T>(nint pointer) where T : unmanaged => pointer != 0
        ? *(T*)pointer : throw new InvalidOperationException("The SDK result has an unexpected model type.");
    internal static string Text(NativeText value)
    {
        if (value.Size == 0) return string.Empty;
        if (value.Data == 0) throw new InvalidOperationException("The SDK returned a null text buffer.");
        return Utf8.GetString(new ReadOnlySpan<byte>((void*)value.Data, checked((int)value.Size)));
    }
    internal static TResult[] Array<TNative, TResult>(NativeSlice value, Func<TNative, TResult> read) where TNative : unmanaged
    {
        if (value.Size == 0) return [];
        if (value.Data == 0) throw new InvalidOperationException("The SDK returned a null model array.");
        int length = checked((int)value.Size);
        var result = new TResult[length];
        var values = new ReadOnlySpan<TNative>((void*)value.Data, length);
        for (int index = 0; index < length; index++) result[index] = read(values[index]);
        return result;
    }
}

// One call owns its unmanaged input storage. The native operation copies the
// validated values into the canonical Rust request before returning.
internal sealed unsafe class InputArena : IDisposable
{
    private static readonly UTF8Encoding Utf8 = new(false, true);
    private readonly List<nint> allocations = [];
    private nuint remaining = 256 * 1024;
    private nint Allocate(nuint bytes)
    {
        if (bytes > remaining) throw new ArgumentException("SDK input exceeds 262144 bytes.");
        remaining -= bytes;
        nint memory = (nint)NativeMemory.Alloc(bytes == 0 ? 1u : bytes);
        try { allocations.Add(memory); }
        catch { NativeMemory.Free((void*)memory); throw; }
        return memory;
    }
    internal NativeText Text(string value)
    {
        ArgumentNullException.ThrowIfNull(value);
        int count = Utf8.GetByteCount(value);
        nint memory = Allocate((nuint)count);
        Utf8.GetBytes(value, new Span<byte>((void*)memory, count));
        return new() { Data = memory, Size = (nuint)count };
    }
    internal NativeSlice Array<TModel, TNative>(TModel[] values, Func<TModel, TNative> write) where TNative : unmanaged
    {
        ArgumentNullException.ThrowIfNull(values);
        nint memory = Allocate(checked((nuint)values.Length * (nuint)sizeof(TNative)));
        var output = new Span<TNative>((void*)memory, values.Length);
        for (int index = 0; index < values.Length; index++) output[index] = write(values[index]);
        return new() { Data = memory, Size = (nuint)values.Length };
    }
    public void Dispose()
    {
        foreach (nint allocation in allocations) NativeMemory.Free((void*)allocation);
        allocations.Clear();
    }
}
