param([Parameter(Mandatory)][string]$ExecutablePath)

$ErrorActionPreference = 'Stop'
$executable = (Resolve-Path -LiteralPath $ExecutablePath).Path
if (Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($executable)) -ErrorAction SilentlyContinue) {
    throw 'Close the existing Transmog instance before running isolated window validation.'
}
$probeRoot = Join-Path (Split-Path -Parent $PSScriptRoot) ('artifacts\window-geometry\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $probeRoot | Out-Null

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class TransmogGeometryProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    private struct MonitorInfo { public int Size; public Rect Monitor, Work; public uint Flags; }
    public sealed class Bounds {
        public int X, Y, Width, Height, InnerWidth, InnerHeight;
        public uint Dpi;
        public bool Maximized;
    }
    private delegate bool MonitorCallback(IntPtr monitor, IntPtr dc, ref Rect rect, IntPtr data);
    private delegate bool WindowCallback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo info);
    [DllImport("user32.dll")] private static extern bool EnumDisplayMonitors(IntPtr dc, IntPtr clip, MonitorCallback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] private static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] private static extern bool IsZoomed(IntPtr window);
    [DllImport("user32.dll")] private static extern bool EnumWindows(WindowCallback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    public static IntPtr MainWindow(uint process) {
        var found = IntPtr.Zero;
        EnumWindows((window, data) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == process && IsWindowVisible(window)) {
                var title = new StringBuilder(512);
                GetWindowText(window, title, title.Capacity);
                if (title.ToString().StartsWith("Transmog ", StringComparison.Ordinal)) found = window;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static Rect[] WorkAreas() { return Areas(true); }
    public static Rect[] DisplayAreas() { return Areas(false); }
    private static Rect[] Areas(bool work) {
        var prior = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            var areas = new List<Rect>();
            EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, (IntPtr monitor, IntPtr dc, ref Rect rect, IntPtr data) => {
                var info = new MonitorInfo { Size = Marshal.SizeOf<MonitorInfo>() };
                if (!GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("Monitor bounds unavailable.");
                areas.Add(work ? info.Work : info.Monitor);
                return true;
            }, IntPtr.Zero);
            return areas.ToArray();
        } finally { SetThreadDpiAwarenessContext(prior); }
    }
    public static Bounds Measure(IntPtr window) {
        var prior = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            Rect outer, inner;
            if (!GetWindowRect(window, out outer) || !GetClientRect(window, out inner))
                throw new InvalidOperationException("Window bounds unavailable.");
            return new Bounds { X = outer.Left, Y = outer.Top, Width = outer.Right - outer.Left,
                Height = outer.Bottom - outer.Top, InnerWidth = inner.Right - inner.Left,
                InnerHeight = inner.Bottom - inner.Top, Dpi = GetDpiForWindow(window), Maximized = IsZoomed(window) };
        } finally { SetThreadDpiAwarenessContext(prior); }
    }
    public static void Place(IntPtr window, int x, int y, int width, int height) {
        var prior = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            if (!SetWindowPos(window, IntPtr.Zero, x, y, width, height, 0x0014))
                throw new InvalidOperationException("Window placement failed.");
        } finally { SetThreadDpiAwarenessContext(prior); }
    }
}
'@

function Open-ProbeWindow {
    $script:probeProcess = Start-Process -FilePath $executable -PassThru -WindowStyle Hidden
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $script:probeProcess.Refresh()
        $handle = [TransmogGeometryProbe]::MainWindow($script:probeProcess.Id)
        if ($script:probeProcess.HasExited) { throw 'The isolated desktop exited before showing its window.' }
        if ($handle -ne [IntPtr]::Zero -and [TransmogGeometryProbe]::IsWindowVisible($handle)) {
            Start-Sleep -Milliseconds 500
            return $handle
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'The isolated desktop did not show its window.'
}

function Close-ProbeWindow([IntPtr]$Handle) {
    if (-not [TransmogGeometryProbe]::PostMessage($Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw 'The native close message was rejected.'
    }
    if (-not $script:probeProcess.WaitForExit(15000)) { throw 'The isolated desktop did not finish closing.' }
    if ($script:probeProcess.ExitCode -ne 0) { throw 'The isolated desktop returned an unsuccessful exit code.' }
}

function Get-BoundsDifference($Expected, $Actual) {
    foreach ($property in @('X', 'Y', 'Width', 'Height', 'InnerWidth', 'InnerHeight', 'Dpi', 'Maximized')) {
        if ($Expected.$property -ne $Actual.$property) { return $property }
    }
    return $null
}

function Wait-ProbeMaximized([IntPtr]$Handle, [bool]$Expected, $ExpectedBounds = $null) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $bounds = [TransmogGeometryProbe]::Measure($Handle)
        if ($bounds.Maximized -eq $Expected -and
            ($null -eq $ExpectedBounds -or $null -eq (Get-BoundsDifference $ExpectedBounds $bounds))) {
            return $bounds
        }
        if ($script:probeProcess.HasExited) { throw 'The isolated desktop exited during a window transition.' }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "The isolated desktop did not reach the expected bounds with maximized=$Expected. Evidence: $probeRoot"
}

function Assert-Bounds($Expected, $Actual, [string]$Context) {
    $property = Get-BoundsDifference $Expected $Actual
    if ($null -ne $property) {
        throw "$Context changed ${property}: expected $($Expected.$property), got $($Actual.$property). Evidence: $probeRoot"
    }
}

function Assert-Visible($Bounds, [string]$Context) {
    foreach ($area in $displays) {
        if ([long]$Bounds.X -lt $area.Right -and [long]$Bounds.X + $Bounds.Width -gt $area.Left -and
            [long]$Bounds.Y -lt $area.Bottom -and [long]$Bounds.Y + $Bounds.Height -gt $area.Top) {
            return
        }
    }
    throw "$Context left the window entirely off screen. Evidence: $probeRoot"
}

function Set-FixtureWindow([int]$X, [int]$Y, [int]$Width, [int]$Height, [bool]$Maximized) {
    # Only edit the owned, closed test profile; never the user's preferences.
    $latest = Get-ChildItem -LiteralPath (Join-Path $probeRoot 'Transmog') -Filter 'preferences.*.json' |
        Sort-Object Name -Descending | Select-Object -First 1
    if (-not $latest) { throw 'The isolated desktop did not save fixture preferences.' }
    $fixture = Get-Content -LiteralPath $latest.FullName -Raw | ConvertFrom-Json
    $fixture.window.x = $X
    $fixture.window.y = $Y
    $fixture.window.width = $Width
    $fixture.window.height = $Height
    $fixture.window.maximized = $Maximized
    $fixture | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $latest.FullName -Encoding utf8NoBOM
}

$priorLocalAppData = $env:LOCALAPPDATA
$priorArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$probeProcess = $null
$evidence = [Collections.Generic.List[object]]::new()
try {
    $env:LOCALAPPDATA = $probeRoot
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $null
    $areas = [TransmogGeometryProbe]::WorkAreas()
    $displays = [TransmogGeometryProbe]::DisplayAreas()
    if ($areas.Count -eq 0) { throw 'Window geometry validation needs a connected display.' }
    $handle = Open-ProbeWindow
    foreach ($area in $areas) {
        # Move first, then size at the target monitor's real DPI. No system display
        # settings change, including when another monitor has negative coordinates.
        [TransmogGeometryProbe]::Place($handle, $area.Left + 40, $area.Top + 50, 960, 720)
        Start-Sleep -Milliseconds 500
        $scale = [TransmogGeometryProbe]::Measure($handle).Dpi / 96.0
        [TransmogGeometryProbe]::Place($handle, $area.Left + 40, $area.Top + 50, [int](960 * $scale), [int](720 * $scale))
        Start-Sleep -Milliseconds 500
        $normal = [TransmogGeometryProbe]::Measure($handle)
        $evidence.Add(@{ Stage = 'placed'; Bounds = $normal })
        for ($cycle = 1; $cycle -le 3; $cycle++) {
            Close-ProbeWindow $handle
            $handle = Open-ProbeWindow
            $actual = [TransmogGeometryProbe]::Measure($handle)
            $evidence.Add(@{ Stage = "reopen-$cycle"; Bounds = $actual })
            Assert-Bounds $normal $actual "Reopen $cycle"
        }
        $null = [TransmogGeometryProbe]::ShowWindowAsync($handle, 3)
        $null = Wait-ProbeMaximized $handle $true
        Close-ProbeWindow $handle
        $handle = Open-ProbeWindow
        $maximized = Wait-ProbeMaximized $handle $true
        $evidence.Add(@{ Stage = 'reopen-maximized'; Bounds = $maximized })
        $null = [TransmogGeometryProbe]::ShowWindowAsync($handle, 9)
        $restored = Wait-ProbeMaximized $handle $false $normal
        $evidence.Add(@{ Stage = 'unmaximize'; Bounds = $restored })
        Assert-Bounds $normal $restored 'Restoring from maximized'

        # A partially visible window is deliberate placement, including on
        # negative-coordinate displays. Startup must preserve its exact bounds.
        [TransmogGeometryProbe]::Place($handle, $area.Left - $normal.Width + 200, $area.Top + 50, $normal.Width, $normal.Height)
        Start-Sleep -Milliseconds 500
        $partial = [TransmogGeometryProbe]::Measure($handle)
        Assert-Visible $partial 'Partially visible fixture'
        Close-ProbeWindow $handle
        $handle = Open-ProbeWindow
        $reopenedPartial = [TransmogGeometryProbe]::Measure($handle)
        $evidence.Add(@{ Stage = 'reopen-partially-visible'; Bounds = $reopenedPartial })
        Assert-Bounds $partial $reopenedPartial 'Reopening a partially visible window'
    }
    Close-ProbeWindow $handle

    # Seed coordinates beyond every connected display to model unplugging a
    # monitor. Verify normal startup and the restored bounds of maximized startup.
    $offscreenX = [int](($displays | Measure-Object -Property Right -Maximum).Maximum + 2000)
    $offscreenY = [int](($displays | Measure-Object -Property Bottom -Maximum).Maximum + 2000)
    $normalRecovery = $null
    foreach ($maximizedIntent in @($false, $true)) {
        Set-FixtureWindow $offscreenX $offscreenY $normal.InnerWidth $normal.InnerHeight $maximizedIntent
        $handle = Open-ProbeWindow
        $recovered = Wait-ProbeMaximized $handle $maximizedIntent
        $evidence.Add(@{ Stage = "offscreen-startup-maximized-$maximizedIntent"; Bounds = $recovered })
        Assert-Visible $recovered 'Recovering off-screen startup'
        if ($maximizedIntent) {
            $null = [TransmogGeometryProbe]::ShowWindowAsync($handle, 9)
            # IsZoomed clears before the restore animation has finished. Wait
            # for the same normal bounds as the identical non-maximized seed.
            $recovered = Wait-ProbeMaximized $handle $false $normalRecovery
            $evidence.Add(@{ Stage = 'offscreen-unmaximize'; Bounds = $recovered })
            Assert-Visible $recovered 'Restoring a recovered maximized window'
        } else {
            $normalRecovery = $recovered
        }
        $recoveredNormal = $recovered
        Close-ProbeWindow $handle
        $handle = Open-ProbeWindow
        $reopenedRecovery = [TransmogGeometryProbe]::Measure($handle)
        $evidence.Add(@{ Stage = 'reopen-recovered'; Bounds = $reopenedRecovery })
        Assert-Bounds $recoveredNormal $reopenedRecovery 'Reopening the recovered window'
        Close-ProbeWindow $handle
    }
    $testedDpi = @($evidence | ForEach-Object { $_.Bounds.Dpi } | Sort-Object -Unique)
    [pscustomobject]@{
        WindowGeometryVerified = $true
        OffscreenStartupRecoveryVerified = $true
        PartiallyVisiblePlacementPreserved = $true
        MaximizedRecoveryRestoreVerified = $true
        TestedDpi = $testedDpi
        ScaledDisplayVerified = @($testedDpi | Where-Object { $_ -gt 96 }).Count -gt 0
        EvidenceRoot = $probeRoot
    }
} finally {
    try {
        $evidence | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $probeRoot 'geometry.json') -Encoding utf8NoBOM
    } finally {
        try {
            if ($probeProcess -and -not $probeProcess.HasExited) {
                Stop-Process -Id $probeProcess.Id -Force
                if (-not $probeProcess.WaitForExit(10000)) { throw 'The owned window probe did not finish stopping.' }
            }
        } finally {
            $env:LOCALAPPDATA = $priorLocalAppData
            $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $priorArguments
        }
    }
}
