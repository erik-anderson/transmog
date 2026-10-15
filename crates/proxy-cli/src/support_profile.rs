//! Side-effect-free profile loading and explicit run-scoped option resolution.
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};
use transmog_capture::support::{EncryptionSuggestion, MAX_PROFILE_BYTES, SupportProfile};
#[derive(Clone)]
pub(crate) struct Loaded {
    pub profile: SupportProfile,
    pub digest: String,
}
impl Loaded {
    pub(crate) fn read(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take((MAX_PROFILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let profile = SupportProfile::parse(&bytes)?;
        let digest = profile.digest()?;
        Ok(Self { profile, digest })
    }
    pub(crate) fn arguments(&self, original: &[String]) -> Vec<String> {
        let mut args = original.to_vec();
        if crate::option(original, "--circular-buffer").is_none()
            && let Some(value) = &self.profile.recording.circular_buffer
        {
            args.extend(["--circular-buffer".into(), value.clone()]);
        }
        if crate::option(original, "--duration").is_none()
            && let Some(value) = self.profile.duration_seconds
        {
            args.extend(["--duration".into(), value.to_string()]);
        }
        if self.profile.recording.include_network_context == Some(true)
            && !args.iter().any(|a| a == "--include-network-context")
        {
            args.push("--include-network-context".into());
        }
        if self.profile.recording.encryption == EncryptionSuggestion::Required
            && !args.iter().any(|a| a == "--encrypt")
        {
            args.push("--encrypt".into());
        }
        args
    }
    pub(crate) fn preflight(&self) {
        println!(
            "Support capture profile: {} · revision {} · SHA-256 {}",
            self.profile.title, self.profile.revision, self.digest
        );
        if let Some(reference) = &self.profile.support_reference {
            println!("Support reference: {reference}");
        }
        if !self.profile.instructions.is_empty() {
            println!("{}", self.profile.instructions);
        }
        for (index, step) in self.profile.steps.iter().enumerate() {
            println!("{}. {step}", index + 1);
        }
        println!(
            "Relevant hosts (advisory only): {}",
            self.profile
                .relevant_hosts
                .iter()
                .map(|h| format!(
                    "{}{}",
                    h.host,
                    if h.include_subdomains {
                        " (and subdomains)"
                    } else {
                        ""
                    }
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("All intercepted traffic may be saved; relevant hosts do not filter recording.");
        if self.profile.suggest_persistent_root {
            println!(
                "The profile suggests a persistent root; only your explicit --persistent-root option can select it."
            );
        }
        if self.profile.recording.encryption == EncryptionSuggestion::Recommended {
            println!("The profile recommends encryption; use --encrypt to select it.");
        }
        for prompt in &self.profile.incident_prompts {
            println!("Incident note prompt: {prompt}");
        }
    }
    pub(crate) fn context(
        &self,
        original: &[String],
        redact: bool,
        body_limit: Option<u64>,
    ) -> serde_json::Value {
        let origin = |flag: &str, profile: bool| {
            if original.iter().any(|a| a == flag) {
                "command line"
            } else if profile {
                "profile"
            } else {
                "built-in"
            }
        };
        let privacy_origin = if original
            .iter()
            .any(|a| matches!(a.as_str(), "--redact" | "--retain-sensitive"))
        {
            "command line (persisted)"
        } else if self.profile.recording.redact_sensitive_headers.is_some() {
            "profile (run only)"
        } else {
            "stored CLI preference / initial default"
        };
        let body_origin = if original.iter().any(|a| {
            matches!(
                a.as_str(),
                "--request-body-limit" | "--unlimited-request-bodies"
            )
        }) {
            "command line (persisted)"
        } else if self.profile.recording.request_body_limit.is_some() {
            "profile (run only)"
        } else {
            "stored CLI preference / initial default"
        };
        serde_json::json!({"version":1,"profile":self.profile,"profileDigest":self.digest,"resolvedSettings":{"redactSensitiveHeaders":{"value":redact,"origin":privacy_origin},"requestBodyLimit":{"value":body_limit,"origin":body_origin},"circularBuffer":{"origin":origin("--circular-buffer",self.profile.recording.circular_buffer.is_some()),"value":crate::option(original,"--circular-buffer").map(str::to_owned).or_else(||self.profile.recording.circular_buffer.clone())},"durationSeconds":{"origin":origin("--duration",self.profile.duration_seconds.is_some()),"value":crate::option(original,"--duration").and_then(|v|v.parse::<u64>().ok()).or(self.profile.duration_seconds)},"networkContext":{"origin":origin("--include-network-context",self.profile.recording.include_network_context.is_some()),"value":original.iter().any(|a|a=="--include-network-context")||self.profile.recording.include_network_context==Some(true)},"encryption":{"origin":origin("--encrypt",self.profile.recording.encryption==EncryptionSuggestion::Required),"value":original.iter().any(|a|a=="--encrypt")||self.profile.recording.encryption==EncryptionSuggestion::Required}},"clientVersionEntered":crate::option(original,"--client-version"),"expectedBehavior":crate::option(original,"--expected"),"actualBehavior":crate::option(original,"--actual")})
    }
}
pub(crate) fn command(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.first().map(String::as_str) != Some("validate") {
        return Err(
            crate::invalid_input("Use profile validate --input FILE [--format json]").into(),
        );
    }
    crate::arguments::validate(&arguments[1..], &["--input", "--format"], &[])?;
    let profile = Loaded::read(Path::new(crate::required_option(arguments, "--input")?))?;
    match crate::option(arguments, "--format").unwrap_or("text") {
        "text" => println!(
            "Valid support capture profile: {} · revision {} · SHA-256 {}",
            profile.profile.title, profile.profile.revision, profile.digest
        ),
        "json" => println!(
            "{}",
            serde_json::json!({"schemaVersion":1,"valid":true,"profileId":profile.profile.id,"revision":profile.profile.revision,"digest":profile.digest})
        ),
        _ => return Err(crate::invalid_input("--format must be text or json").into()),
    }
    Ok(())
}
