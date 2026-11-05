param([Parameter(Mandatory)][int]$ProbeProcessId)
$ErrorActionPreference = 'Stop'
# Native key messages exercise WebView2 accelerators even when the test session
# has no foreground input desktop. Only address the owned probe's focused child.
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class TransmogProbeKey {
    [StructLayout(LayoutKind.Sequential)]
    public struct GuiThreadInfo {
        public uint size, flags;
        public IntPtr active, focus, capture, menuOwner, moveSize, caret;
        public int left, top, right, bottom;
    }
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool GetGUIThreadInfo(uint thread, ref GuiThreadInfo info);
    [DllImport("user32.dll")]
    public static extern bool IsChild(IntPtr parent, IntPtr child);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessage(IntPtr window, uint message, UIntPtr key, IntPtr data);
}
'@
$probe = Get-Process -Id $ProbeProcessId -ErrorAction Stop
try {
    $null = $probe.Handle
    $window = $probe.MainWindowHandle
    [uint32]$owner = 0
    $thread = [TransmogProbeKey]::GetWindowThreadProcessId($window, [ref]$owner)
    if ($owner -ne $ProbeProcessId -or $probe.HasExited) { throw 'The window is not owned by the probe.' }
    $info = New-Object TransmogProbeKey+GuiThreadInfo
    $info.size = [Runtime.InteropServices.Marshal]::SizeOf($info)
    if (-not [TransmogProbeKey]::GetGUIThreadInfo($thread, [ref]$info)) { throw 'Probe focus is unavailable.' }
    if ($info.focus -eq [IntPtr]::Zero -or -not [TransmogProbeKey]::IsChild($window, $info.focus)) {
        throw 'The focused control is not inside the owned probe window.'
    }
    if (-not [TransmogProbeKey]::PostMessage($info.focus, 0x100, [UIntPtr]0x7b, [IntPtr]0x00580001) -or
        -not [TransmogProbeKey]::PostMessage($info.focus, 0x101, [UIntPtr]0x7b, [IntPtr]([long]0xc0580001))) {
        throw 'Could not post F12 to the owned probe.'
    }
    [ordered]@{ window=$window.ToInt64(); focus=$info.focus.ToInt64(); key='F12' } | ConvertTo-Json -Compress
} finally { $probe.Dispose() }
