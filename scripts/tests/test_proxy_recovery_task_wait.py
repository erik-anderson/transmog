"""Exercise the actual Scheduler observer without tasks, apps, or proxy changes."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "apps/desktop/tests/support/run_proxy_recovery_task.ps1"
SHELL = shutil.which("powershell") or shutil.which("pwsh")
FIXTURE = r"""
$ErrorActionPreference = 'Stop'
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$trigger = [pscustomobject]@{ Type = 9; UserId = $sid }
$triggers = [pscustomobject]@{ Count = 1 }
$triggers | Add-Member ScriptMethod Item { param($index) $script:trigger }
$action = [pscustomobject]@{ Arguments = '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand fixture' }
$actions = [pscustomobject]@{ Count = 1 }
$actions | Add-Member ScriptMethod Item { param($index) $script:action }
$settings = [pscustomobject]@{ DisallowStartIfOnBatteries = $false; StopIfGoingOnBatteries = $false; RunOnlyIfNetworkAvailable = $false; RunOnlyIfIdle = $false; ExecutionTimeLimit = $env:FIXTURE_TASK_LIMIT }
$definition = [pscustomobject]@{ Principal = [pscustomobject]@{ UserId = $sid; LogonType = 3; RunLevel = 0 }; Triggers = $triggers; Actions = $actions; Settings = $settings }
$running = [pscustomobject]@{ State = 2; EnginePID = 123; Clock = [Diagnostics.Stopwatch]::new() }
$running | Add-Member ScriptMethod Refresh {
    $this.State = if ($env:FIXTURE_STUCK -eq '1') { 4 } elseif ($this.Clock.Elapsed.TotalSeconds -ge [double]$env:FIXTURE_FINISH) { 3 } elseif ($this.Clock.Elapsed.TotalSeconds -ge 0.1) { 4 } else { 2 }
    if ($env:FIXTURE_REFRESH_ERROR) { throw [Runtime.InteropServices.COMException]::new('Fixture Scheduler error', [int]$env:FIXTURE_REFRESH_ERROR) }
}
$task = [pscustomobject]@{ Definition = $definition; Xml = '<Task />' }
$task | Add-Member ScriptMethod Run { param($parameters) $script:running.Clock.Restart(); $script:running }
$script:reportReads = 0
$task | Add-Member ScriptProperty LastTaskResult { $script:reportReads++; if ($script:running.State -eq 3 -and $script:reportReads -gt [int]$env:FIXTURE_PENDING_READS) { [int]$env:FIXTURE_EXIT } else { 267009 } }
$folder = [pscustomobject]@{}
$folder | Add-Member ScriptMethod GetTask { param($name) if ($env:FIXTURE_REMOVED -eq '1' -and $script:running.Clock.IsRunning) { throw [Runtime.InteropServices.COMException]::new('Fixture task removed', -2147024894) }; $script:task }
$service = [pscustomobject]@{}
$service | Add-Member ScriptMethod Connect { }
$service | Add-Member ScriptMethod GetFolder { param($path) $script:folder }
function New-Object { param([string]$ComObject) if ($ComObject -cne 'Schedule.Service') { throw 'Unexpected COM request.' }; $script:service }
"""


@unittest.skipUnless(os.name == "nt" and SHELL, "Windows identity and PowerShell Scheduler fixture")
class ProxyRecoveryTaskWaitTests(unittest.TestCase):
    def observe(self, *, timeout=0.75, finish=0.5, stuck=False, exit_code=0, task_limit="PT1M", removed=False, refresh_error="", pending_reads=0):
        source = SCRIPT.read_text()
        self.assertEqual(source.count("$timeoutSeconds = 75"), 1)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = FIXTURE + source.replace("$timeoutSeconds = 75", f"$timeoutSeconds = {timeout}")
            env = dict(os.environ, TRANSMOG_TEST_TASK="fixture", TRANSMOG_TEST_TASK_DIAGNOSTICS=str(root / "evidence"),
                       FIXTURE_TASK_LIMIT=task_limit, FIXTURE_FINISH=str(finish), FIXTURE_STUCK=str(int(stuck)), FIXTURE_EXIT=str(exit_code),
                       FIXTURE_REMOVED=str(int(removed)), FIXTURE_REFRESH_ERROR=str(refresh_error), FIXTURE_PENDING_READS=str(pending_reads))
            result = subprocess.run([SHELL, "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", fixture],
                                    env=env, capture_output=True, text=True, encoding="utf-8", timeout=15)
            records = [json.loads(p.read_text(encoding="utf-8-sig")) for p in (root / "evidence").glob("*.json")]
            return result, records

    def test_completion_after_the_former_half_task_budget_remains_required(self):
        rejected, old = self.observe(timeout=0.3)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(old[0]["result"], "deadline")
        completed, evidence = self.observe()
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(evidence[0]["result"], "completed")
        self.assertGreaterEqual(evidence[0]["elapsedSeconds"], 0.5)
        self.assertEqual(evidence[0]["lastTaskResult"], 0)

    def test_a_stuck_task_still_fails_and_retains_state(self):
        result, evidence = self.observe(stuck=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(evidence[0]["result"], "deadline")
        self.assertEqual(evidence[0]["samples"][-1]["state"], 4)
        self.assertEqual(evidence[0]["lastTaskResult"], 267009)

    def test_unsuccessful_completion_is_not_accepted(self):
        result, evidence = self.observe(finish=0.1, exit_code=42)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(evidence[0]["result"], "failed")
        self.assertEqual(evidence[0]["lastTaskResult"], 42)

    def test_an_unexpected_longer_task_limit_is_rejected(self):
        result, evidence = self.observe(task_limit="PT2M")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("one-minute recovery task execution limit", result.stderr)
        self.assertFalse(evidence)

    def test_documented_completion_and_self_removal_are_accepted(self):
        result, evidence = self.observe(removed=True, refresh_error=-2147216629)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(evidence[0]["result"], "completed")
        self.assertIsNone(evidence[0]["lastTaskResult"])

    def test_unexpected_scheduler_errors_fail_closed(self):
        result, evidence = self.observe(refresh_error=-2147467259)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Fixture Scheduler error", result.stderr)
        self.assertFalse(evidence)

    def test_completion_reporting_uses_the_same_finite_budget(self):
        result, evidence = self.observe(finish=0.1, pending_reads=2)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(evidence[0]["lastTaskResult"], 0)
        result, evidence = self.observe(finish=0.1, pending_reads=100)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(evidence[0]["result"], "deadline")


if __name__ == "__main__":
    unittest.main()
