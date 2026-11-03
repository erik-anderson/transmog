param([Parameter(Mandatory)][int]$ProbeProcessId, [Parameter(Mandatory)][string]$WindowTitle,
    [ValidateRange(1, 120)][int]$ReadySeconds = 30)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Collections.Generic;
using System.Text;
public static class TransmogProbeWindow {
    private delegate bool WindowCallback(IntPtr window, IntPtr state);
    [DllImport("user32.dll")]
    private static extern bool EnumWindows(WindowCallback callback, IntPtr state);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr SendMessageTimeout(IntPtr window, uint message,
        UIntPtr wparam, IntPtr lparam, uint flags, uint timeout, out UIntPtr result);
    public static bool SendBounded(IntPtr window, uint message, uint timeout) {
        UIntPtr result;
        // SMTO_BLOCK | SMTO_ABORTIFHUNG; never send an unbounded synchronous message.
        return SendMessageTimeout(window, message, UIntPtr.Zero, IntPtr.Zero, 3, timeout, out result) != IntPtr.Zero;
    }
    public static Dictionary<IntPtr,string> OwnedWindows(uint expected) {
        var windows = new Dictionary<IntPtr,string>();
        EnumWindows((window, state) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == expected) {
                var text = new StringBuilder(512);
                GetWindowText(window, text, text.Capacity);
                if (text.Length > 0) windows[window] = text.ToString();
            }
            return true;
        }, IntPtr.Zero);
        return windows;
    }
}
'@
$probe = Get-Process -Id $ProbeProcessId -ErrorAction Stop
try {
    # Retain the process identity while discovering and messaging its window.
    $null = $probe.Handle
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $window = [IntPtr]::Zero
    do {
        if ($probe.HasExited) { throw 'The owned desktop exited before its window was ready.' }
        $owned = [TransmogProbeWindow]::OwnedWindows($ProbeProcessId)
        $matchingWindows = @($owned.GetEnumerator() | Where-Object {
            $_.Value -eq $WindowTitle -or ($WindowTitle -eq 'Transmog' -and $_.Value -match '^Transmog [0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?(?: .*)?$')
        })
        if ($WindowTitle -eq 'Transmog' -and $matchingWindows.Count -gt 1) { throw 'The isolated main window title was ambiguous.' }
        $match = $matchingWindows | Select-Object -First 1
        $remaining = [Math]::Max(1, [Math]::Ceiling($ReadySeconds * 1000 - $clock.Elapsed.TotalMilliseconds))
        if ($match -and [TransmogProbeWindow]::IsWindowVisible($match.Key) -and
            [TransmogProbeWindow]::IsWindowEnabled($match.Key) -and
            [TransmogProbeWindow]::SendBounded($match.Key, 0, [uint32][Math]::Min(1000, $remaining))) {
            $window = $match.Key
            break
        }
        Start-Sleep -Milliseconds 50
    } while ($clock.Elapsed.TotalSeconds -lt $ReadySeconds)
    if ($window -eq [IntPtr]::Zero) { throw "The owned probe window was not ready. Owned titles: $($owned.Values -join ', ')" }
    [uint32]$owner = 0
    $null = [TransmogProbeWindow]::GetWindowThreadProcessId($window, [ref]$owner)
    if ($owner -ne $ProbeProcessId -or $probe.HasExited) { throw 'The window no longer belongs to the isolated probe process.' }
    # Exercise native CloseRequested and acknowledge its message processing before
    # the caller starts its separate process-shutdown deadline.
    if (-not [TransmogProbeWindow]::SendBounded($window, 0x0010, 10000) -and -not $probe.WaitForExit(1000)) {
        throw 'The native close message was not processed within its deadline.'
    }
} finally {
    $probe.Dispose()
}
