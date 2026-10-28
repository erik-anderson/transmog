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
            Connector::new(
                root,
                key.to_string(),
                arguments.iter().any(|arg| arg == "--start-app"),
            )
            .serve()
            .await?;
        }
        Some("status") if arguments.len() == 1 => {
            require_key(&key)?;
            let result = LocalClient::new(root)
                .call(&key, "transmog_status", serde_json::json!({}))
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("call") if arguments.len() == 2 => {
            require_key(&key)?;
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
            let result = LocalClient::new(root)
                .call(&key, &arguments[1], input)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => return Err(super::invalid_input("use transmog-cli mcp --help").into()),
    }
    Ok(())
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
