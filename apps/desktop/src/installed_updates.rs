//! Installer-only maintenance and nonblocking retired executable cleanup.

pub(crate) fn maintenance_exit_code() -> Option<i32> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments
        .first()
        .is_none_or(|value| value != "--apply-update-payload")
    {
        return None;
    }
    let result = (|| {
        if arguments.len() != 3 {
            return Err(std::io::Error::other(
                "Expected installation and payload directories",
            ));
        }
        transmog_install_support::apply(
            std::path::Path::new(&arguments[1]),
            std::path::Path::new(&arguments[2]),
        )
    })();
    match result {
        Ok(cleanup) => {
            if cleanup.notification_failed {
                eprintln!(
                    "The updated CLI could not notify existing MCP connectors. Restart the agent's MCP connection."
                );
            }
            if cleanup.deferred > 0 {
                eprintln!("Retired executable cleanup deferred until a later Transmog start.");
            }
            Some(0)
        }
        Err(error) => {
            eprintln!("Transmog executable replacement failed: {error}");
            Some(2)
        }
    }
}

pub(crate) fn cleanup(application: transmog_app::Application) {
    let Some(root) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_owned))
    else {
        return;
    };
    if !root.join(".transmog-retired").exists() {
        return;
    }
    // File access/antivirus can be slow. Startup and the UI never await this.
    std::thread::spawn(move || {
        if let Ok(result) = transmog_install_support::cleanup(&root)
            && result.deferred > 0
        {
            application.record_diagnostic(transmog_app::DiagnosticLevel::Info, "updater", "retired-cleanup-deferred", "Some retired executable files are still in use. Cleanup will retry on a later start.");
        }
    });
}

pub(crate) fn ready() -> Result<Option<std::fs::File>, ()> {
    let Ok(executable) = std::env::current_exe() else {
        return Err(());
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match transmog_install_support::desktop_guard(&executable) {
            Ok(guard) => return Ok(guard),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => {
                rfd::MessageDialog::new().set_title("Transmog update needs attention")
                    .set_description(format!("Transmog could not finish starting: {error}\n\nWait for the installer to finish, or run it again to recover the update."))
                    .set_level(rfd::MessageLevel::Error).show();
                return Err(());
            }
        }
    }
}
