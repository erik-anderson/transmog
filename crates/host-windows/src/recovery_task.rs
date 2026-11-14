use std::path::{Path, PathBuf};

use crate::{WindowsHostError, WindowsProxyIntegration, run_script_bounded};

/// Best-effort login recovery registration, separate from proxy restoration.
pub trait ProxyRecoveryTask: Send + Sync {
    /// Arms recovery before changing proxy settings.
    ///
    /// # Errors
    /// Returns a registration failure; callers must still allow proxy use.
    fn register(&self) -> Result<(), WindowsHostError>;

    /// Removes the task after successful restoration, including stale tasks.
    ///
    /// # Errors
    /// Returns a cleanup failure; restored networking remains usable.
    fn remove(&self) -> Result<(), WindowsHostError>;

    /// Removes the task using the journal path saved by its original owner.
    ///
    /// Path-keyed registrations override this to handle recovery through a
    /// directory alias. Other registrations use their ordinary cleanup.
    ///
    /// # Errors
    /// Returns a cleanup failure; restored networking remains usable.
    fn remove_for_journal(&self, _journal: &Path) -> Result<(), WindowsHostError> {
        self.remove()
    }
}

/// A passwordless, current-user Task Scheduler logon recovery task.
#[derive(Clone, Debug)]
pub struct ScheduledProxyRecovery {
    journal: PathBuf,
    executable: PathBuf,
}

impl ScheduledProxyRecovery {
    /// Creates a task invoking the executable's headless `--recover-proxy` mode.
    pub fn new(journal: impl Into<PathBuf>, executable: impl Into<PathBuf>) -> Self {
        Self {
            journal: journal.into(),
            executable: executable.into(),
        }
    }

    /// Returns the task name scoped to the current user and absolute journal path.
    ///
    /// # Errors
    /// Returns a Windows command or path failure.
    pub fn name(&self) -> Result<String, WindowsHostError> {
        self.command("name")
    }

    /// Restores pending tasks belonging to an executable before it is removed.
    ///
    /// Task Scheduler enumeration failures are nonfatal, just like registration
    /// failures. This includes CLI captures using a custom journal directory.
    ///
    /// # Errors
    /// Returns a proxy restoration failure, retaining its journal and task.
    pub fn recover_for_executable(executable: &Path) -> Result<(), WindowsHostError> {
        let task = Self::new(executable.with_extension("json"), executable);
        let journals: Vec<PathBuf> = match task
            .command("journals")
            .and_then(|output| serde_json::from_str(&output).map_err(WindowsHostError::Journal))
        {
            Ok(journals) => journals,
            Err(error) => {
                eprintln!("Windows proxy recovery task enumeration failed: {error}");
                return Ok(());
            }
        };
        for journal in journals {
            WindowsProxyIntegration::system(journal).recover_pending()?;
        }
        Ok(())
    }

    fn command(&self, operation: &str) -> Result<String, WindowsHostError> {
        let input = serde_json::json!({
            "operation": operation,
            "journal": std::path::absolute(&self.journal).map_err(WindowsHostError::Io)?,
            "executable": std::path::absolute(&self.executable).map_err(WindowsHostError::Io)?,
        });
        run_script_bounded(
            include_str!("recovery_task.ps1"),
            Some(&input.to_string()),
            std::time::Duration::from_secs(10),
        )
        .map(|output| output.trim().to_owned())
    }
}

impl ProxyRecoveryTask for ScheduledProxyRecovery {
    fn register(&self) -> Result<(), WindowsHostError> {
        self.command("register").map(|_| ())
    }

    fn remove(&self) -> Result<(), WindowsHostError> {
        self.command("remove").map(|_| ())
    }

    fn remove_for_journal(&self, journal: &Path) -> Result<(), WindowsHostError> {
        Self::new(journal, &self.executable).remove()
    }
}

/// Handles the headless recovery command before application or UI initialization.
///
/// Returns `None` for ordinary launches, or an exit code for recovery. The command
/// uses the same journal, lease, restoration and task cleanup as normal shutdown.
pub fn recovery_command_exit_code() -> Option<i32> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments
        .first()
        .is_none_or(|argument| argument != "--recover-proxy")
    {
        return None;
    }
    Some(
        if arguments.len() == 2 && Path::new(&arguments[1]).is_absolute() {
            match WindowsProxyIntegration::system(Path::new(&arguments[1])).recover_pending() {
                Ok(_) | Err(WindowsHostError::ProxyInUse) => 0,
                Err(error) => {
                    eprintln!("Windows proxy login recovery failed: {error}");
                    2
                }
            }
        } else {
            2
        },
    )
}
