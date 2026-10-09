param([Parameter(Mandatory)][int]$ProbeProcessId, [Parameter(Mandatory)][string]$WindowTitle)
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
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
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
$owned = [TransmogProbeWindow]::OwnedWindows($ProbeProcessId)
$matchingWindows = @($owned.GetEnumerator() | Where-Object {
    $_.Value -eq $WindowTitle -or ($WindowTitle -eq 'Transmog' -and $_.Value -match '^Transmog [0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?(?: .*)?$')
})
$match = $matchingWindows | Select-Object -First 1
if ($WindowTitle -eq 'Transmog' -and $matchingWindows.Count -ne 1) { throw 'The isolated main window title was missing or ambiguous.' }
if (-not $match) { throw "The requested native probe window was not found. Owned titles: $($owned.Values -join ', ')" }
$window = $match.Key
[uint32]$owner = 0
$null = [TransmogProbeWindow]::GetWindowThreadProcessId($window, [ref]$owner)
if ($owner -ne $ProbeProcessId) { throw 'The window does not belong to the isolated probe process.' }
# Exercise the native CloseRequested path; CDP Page.close only closes its webview.
if (-not [TransmogProbeWindow]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) {
    throw 'The native close message was rejected.'
}
