//! Opt-in native acceptance: execute the real logon task through Task Scheduler.

#![cfg(windows)]

use std::{fs::OpenOptions, os::windows::process::CommandExt as _, process::Command};

use transmog_host_windows::{
    ProxyRecoveryTask, ProxySettings, ProxySettingsBackend, ScheduledProxyRecovery,
    SystemProxyBackend,
};

struct RestoreHost {
    settings: ProxySettings,
    task: ScheduledProxyRecovery,
}

impl Drop for RestoreHost {
    fn drop(&mut self) {
        // Also restore on assertion failure, and never leave a test task armed.
        if let Ok(name) = self.task.name() {
            let _ = scheduler_output(
                r"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'; $service.Connect()
try { $task = $service.GetFolder('\').GetTask($env:TRANSMOG_TEST_TASK) }
catch { if ($_.Exception.HResult -eq -2147024894) { exit 0 }; throw }
$task.Stop(0)
$deadline = [DateTime]::UtcNow.AddSeconds(10)
while ($task.GetInstances(0).Count -gt 0 -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
",
                &name,
            );
        }
        let _ = SystemProxyBackend.write(&self.settings);
        let _ = SystemProxyBackend.notify_changed();
        let _ = self.task.remove();
    }
}

fn scheduler_output(script: &str, name: &str) -> std::io::Result<std::process::Output> {
    Command::new("powershell.exe")
        .creation_flags(0x0800_0000)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("TRANSMOG_TEST_TASK", name)
        .output()
}

fn scheduler(script: &str, name: &str) {
    let output = scheduler_output(script, name).unwrap();
    assert!(
        output.status.success(),
        "Task Scheduler API check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

const RUN_TASK: &str = r"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$task = $service.GetFolder('\').GetTask($env:TRANSMOG_TEST_TASK)
$definition = $task.Definition
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
function Get-AccountSid([string]$account) {
    if ($account.StartsWith('S-1-')) { return $account }
    return ([Security.Principal.NTAccount]::new($account)).Translate([Security.Principal.SecurityIdentifier]).Value
}
if ((Get-AccountSid $definition.Principal.UserId) -cne $sid -or $definition.Principal.LogonType -ne 3 -or $definition.Principal.RunLevel -ne 0) { throw 'Recovery must run as the current user without elevation or a password.' }
if ($definition.Triggers.Count -ne 1 -or $definition.Triggers.Item(1).Type -ne 9 -or (Get-AccountSid $definition.Triggers.Item(1).UserId) -cne $sid) { throw 'Expected a logon trigger scoped to this user.' }
if ($definition.Settings.DisallowStartIfOnBatteries -or $definition.Settings.StopIfGoingOnBatteries -or $definition.Settings.RunOnlyIfNetworkAvailable -or $definition.Settings.RunOnlyIfIdle) { throw 'Recovery must run on battery without networking or idle requirements.' }
if ($definition.Actions.Item(1).Arguments -notlike '*-WindowStyle Hidden -EncodedCommand *') { throw 'Expected the encoded, hidden headless recovery action.' }
# Invoke IRegisteredTask.Run, rather than starting the action's executable ourselves.
$running = $task.Run($null)
$deadline = [DateTime]::UtcNow.AddSeconds(30)
do {
    Start-Sleep -Milliseconds 100
    try {
        $running.Refresh()
        $active = $running.State -in @(2, 4) # Queued or running.
    } catch {
        if ($_.Exception.HResult -ne -2147216629) { throw } # SCHED_E_TASK_NOT_RUNNING.
        $active = $false
    }
} while ($active -and [DateTime]::UtcNow -lt $deadline)
if ($active) { throw 'The recovery task did not finish.' }
";

const ASSERT_REMOVED: &str = r"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'; $service.Connect()
try { $null = $service.GetFolder('\').GetTask($env:TRANSMOG_TEST_TASK) }
catch { if ($_.Exception.HResult -eq -2147024894) { exit 0 }; throw }
throw 'Recovery left its task registered.'
";

const HIDE_PRIOR_LAUNCHER: &str = r"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'; $service.Connect()
$folder = $service.GetFolder('\')
$definition = $folder.GetTask($env:TRANSMOG_TEST_TASK).Definition
$definition.Settings.Hidden = $true
$action = $definition.Actions.Item(1)
$prefix = '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand '
$launcher = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($action.Arguments.Substring($prefix.Length)))
$launcher = '# Previous launcher version' + [Environment]::NewLine + $launcher
$action.Arguments = $prefix + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($launcher))
$data = $definition.RegistrationInfo.Documentation | ConvertFrom-Json
$data.actionArguments = $action.Arguments
$definition.RegistrationInfo.Documentation = $data | ConvertTo-Json -Compress
$null = $folder.RegisterTaskDefinition($env:TRANSMOG_TEST_TASK, $definition, 6, $definition.Principal.UserId, $null, 3)
";

fn preserves_later_bypass(
    journal: &std::path::Path,
    before: &ProxySettings,
    task: &ScheduledProxyRecovery,
    name: &str,
) {
    let mut applied = before.clone();
    applied.bypass = Some("transmog-applied-test.invalid".into());
    let mut later = before.clone();
    later.bypass = Some(format!(
        "{};transmog-user-edit-test.invalid",
        before.bypass.as_deref().unwrap_or_default()
    ));
    std::fs::write(
        journal,
        serde_json::to_vec(
            &serde_json::json!({ "schema": 1, "prior": before, "applied": applied }),
        )
        .unwrap(),
    )
    .unwrap();
    task.register().unwrap();
    SystemProxyBackend.write(&later).unwrap();
    SystemProxyBackend.notify_changed().unwrap();
    scheduler(RUN_TASK, name);
    assert_eq!(SystemProxyBackend.read().unwrap(), later);
    assert!(!journal.exists());
    scheduler(ASSERT_REMOVED, name);
}

#[test]
#[ignore = "registers and runs a real task; temporarily changes only the current-user proxy bypass list"]
fn scheduler_runs_headless_recovery_and_cleans_up_after_an_interrupted_run() {
    let directory = tempfile::Builder::new()
        .prefix("Transmog's recovery Ω %USERNAME% $(Arg0) ")
        .tempdir()
        .unwrap();
    let journal = directory.path().join("proxy-recovery.json");
    let executable = directory.path().join("Transmog recovery test.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_transmog"), &executable).unwrap();
    let task = ScheduledProxyRecovery::new(&journal, &executable);
    let before = SystemProxyBackend.read().unwrap();
    let product_state =
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Transmog");
    assert!(
        !product_state.join("proxy-owner-v1.json").exists()
            && !product_state.join("proxy-recovery-v1.json").exists(),
        "Close or recover the existing proxy before running this native test"
    );
    let guard = RestoreHost {
        settings: before.clone(),
        task,
    };
    let name = guard.task.name().unwrap();
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(journal.with_extension("lock"))
        .unwrap();
    lease.try_lock().unwrap();
    std::fs::write(
        &journal,
        serde_json::to_vec(&serde_json::json!({ "schema": 1, "prior": before })).unwrap(),
    )
    .unwrap();
    guard.task.register().unwrap();
    let mut during = before.clone();
    // This reserved hostname cannot affect ordinary traffic. Keep proxy enable
    // and server settings unchanged while proving a real registry restoration.
    during.bypass = Some(format!(
        "{};transmog-proxy-recovery-test.invalid",
        before.bypass.as_deref().unwrap_or_default()
    ));
    SystemProxyBackend.write(&during).unwrap();
    SystemProxyBackend.notify_changed().unwrap();

    scheduler(RUN_TASK, &name);
    assert!(journal.exists(), "Recovery must leave a live proxy alone");
    assert_eq!(SystemProxyBackend.read().unwrap(), during);
    scheduler(
        r"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'; $service.Connect()
if ($service.GetFolder('\').GetTask($env:TRANSMOG_TEST_TASK).LastTaskResult -ne 0) { throw 'The live-run check did not execute successfully.' }
",
        &name,
    );

    drop(lease); // Simulate Windows releasing locks after a hard termination.
    scheduler(RUN_TASK, &name);
    assert!(
        !journal.exists(),
        "The scheduled executable must consume the recovery journal"
    );
    assert_eq!(SystemProxyBackend.read().unwrap(), before);
    scheduler(ASSERT_REMOVED, &name);
    preserves_later_bypass(&journal, &before, &guard.task, &name);

    // Recovery through a directory alias must remove the task keyed by the
    // original owner's journal path, not the recovering adapter's path.
    guard.task.register().unwrap();
    ScheduledProxyRecovery::new(directory.path().join("alias-proxy.json"), &executable)
        .remove_for_journal(&journal)
        .unwrap();
    scheduler(ASSERT_REMOVED, &name);

    // Installer maintenance also removes a stale task with no journal, and
    // recognizes the executable even outside its usual installation directory.
    guard.task.register().unwrap();
    scheduler(HIDE_PRIOR_LAUNCHER, &name);
    ScheduledProxyRecovery::recover_for_executable(&executable).unwrap();
    scheduler(ASSERT_REMOVED, &name);
}
