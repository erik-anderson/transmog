use serde_json::Value;
use std::{
    error::Error,
    io::{self, IsTerminal as _, Read as _},
    path::PathBuf,
};
use transmog_mcp::{Connector, LocalClient};

pub(crate) async fn command(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let root = state_root()?;
    let key = zeroize::Zeroizing::new(std::env::var("TRANSMOG_ACCESS_KEY").unwrap_or_default());
    match arguments.first().map(String::as_str) {
        Some("serve") => {
            super::arguments::validate(&arguments[1..], &[], &["--start-app"])?;
            let client = ClientLifetime::capture();
            let ended = client.ended();
            tokio::pin!(ended);
            let executable = std::env::current_exe()?;
            let installation = tokio::select! {
                result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    loop {
                        match transmog_install_support::InstallationWatch::capture(&executable) {
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                            result => return result,
                        }
                    }
                }) => result.map_err(|_| io::Error::other("Installation is still in progress. Retry the MCP connection after it finishes."))??,
                () = &mut ended => return Ok(()),
            };
            let connector = Connector::new(
                root,
                key.to_string(),
                arguments.iter().any(|arg| arg == "--start-app"),
            );
            tokio::select! {
                result = connector.serve() => result?,
                () = &mut ended => tracing::info!("MCP connector stopped: client process exited"),
                () = async {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if installation.replaced() { break; }
                    }
                } => tracing::info!("MCP connector stopped: installed CLI was replaced; reconnect using the updated executable"),
            }
        }
        Some("status") if arguments.len() == 1 => {
            require_key(&key)?;
            let result = LocalClient::new(root)
                .call(&key, "transmog_status", serde_json::json!({}))
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("call") if arguments.len() == 2 => {
            if !transmog_mcp::tool_catalog()
                .iter()
                .any(|tool| tool.name == arguments[1])
            {
                return Err(super::invalid_input(
                    "unknown Transmog tool; use MCP tools/list to discover operations",
                )
                .into());
            }
            let input = if io::stdin().is_terminal() {
                serde_json::json!({})
            } else {
                let mut bytes = Vec::new();
                io::stdin().take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 1024 * 1024 {
                    return Err(super::invalid_input("operation arguments exceed 1 MiB").into());
                }
                if bytes.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_slice::<Value>(&bytes)
                        .map_err(|_| super::invalid_input("stdin must contain a JSON object"))?
                }
            };
            if !input.is_object() {
                return Err(super::invalid_input("stdin must contain a JSON object").into());
            }
            let result = Connector::new(root, key.to_string(), false)
                .call(&arguments[1], input)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => return Err(super::invalid_input("use transmog-cli mcp --help").into()),
    }
    Ok(())
}

/// Process identity includes its creation time so PID reuse cannot extend a
/// connector's lifetime. No command lines or environment secrets are inspected.
struct ClientLifetime {
    system: sysinfo::System,
    identity: Option<(sysinfo::Pid, u64)>,
}

impl ClientLifetime {
    fn capture() -> Self {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
        let mut system = System::new();
        let own_pid = Pid::from_u32(std::process::id());
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[own_pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        let parent = system.process(own_pid).and_then(sysinfo::Process::parent);
        let identity = parent.and_then(|pid| {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing(),
            );
            system
                .process(pid)
                .map(|process| (pid, process.start_time()))
        });
        Self { system, identity }
    }

    async fn ended(mut self) {
        let Some((pid, started)) = self.identity else {
            // Some restricted environments cannot inspect the parent. Stdio
            // and desktop lifetime still apply; do not reject valid clients.
            std::future::pending::<()>().await;
            return;
        };
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            self.system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            if self.system.process(pid).is_none_or(|process| {
                process.start_time() != started
                    || process.status() == sysinfo::ProcessStatus::Zombie
            }) {
                return;
            }
        }
    }
}

fn require_key(key: &str) -> io::Result<()> {
    if key.is_empty() {
        Err(super::invalid_input(
            "supply TRANSMOG_ACCESS_KEY from this agent's configuration; create a key in Transmog Settings → Integrations → MCP",
        ))
    } else {
        Ok(())
    }
}

fn state_root() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|root|root.is_absolute())
            .map(|root| root.join("Transmog"))
            .ok_or_else(|| super::invalid_input("Configure the absolute LOCALAPPDATA entry from Transmog's generated agent configuration."))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|root| PathBuf::from(root).join(".local/state"))
            })
            .map(|root| root.join("Transmog"))
            .ok_or_else(|| {
                super::invalid_input("the local Transmog state directory is unavailable")
            })
    }
}
