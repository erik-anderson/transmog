//! Run-scoped inspection choices shared by record and serve.
use std::io;
use transmog_core::process_inspection::{ProcessInspectionMode, ProcessInspectionPolicy};

pub(crate) const PROCESS_OPTIONS: &[&str] = &["--process", "--exclude-process"];

pub(crate) fn process_policy(arguments: &[String]) -> io::Result<ProcessInspectionPolicy> {
    let names = |name: &str| {
        crate::arguments::options(arguments)
            .filter(|(option, _)| *option == name)
            .filter_map(|(_, value)| value.map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let include = names("--process");
    let exclude = names("--exclude-process");
    if !include.is_empty() && !exclude.is_empty() {
        return Err(crate::invalid_input(
            "--process and --exclude-process cannot be combined; choose one process inspection policy",
        ));
    }
    let mode = if !include.is_empty() {
        ProcessInspectionMode::Only
    } else if !exclude.is_empty() {
        ProcessInspectionMode::Except
    } else {
        ProcessInspectionMode::All
    };
    #[cfg(not(any(windows, target_os = "linux")))]
    if mode != ProcessInspectionMode::All {
        return Err(crate::invalid_input(
            "Local process attribution is unavailable on this platform; --process and --exclude-process are not supported.",
        ));
    }
    ProcessInspectionPolicy {
        mode,
        include,
        exclude,
    }
    .validated()
    .map_err(|error| crate::invalid_input(error.to_string()))
}

pub(crate) fn decrypt_https(arguments: &[String]) -> bool {
    !crate::arguments::has_flag(arguments, "--no-decrypt-https")
}

pub(crate) fn print_policy(policy: &ProcessInspectionPolicy, decrypt: bool) {
    let selection = match policy.mode {
        ProcessInspectionMode::All => "All processes".to_owned(),
        ProcessInspectionMode::Only => format!("Only {}", policy.include.join(", ")),
        ProcessInspectionMode::Except => format!("All except {}", policy.exclude.join(", ")),
    };
    println!("Process inspection: {selection}.");
    if policy.mode != ProcessInspectionMode::All {
        println!(
            "Names match executable basenames. Unidentified and remote callers pass through without inspection or recording."
        );
        #[cfg(windows)]
        println!(
            "Process names are case insensitive; extensionless names accept an omitted .exe suffix."
        );
        #[cfg(target_os = "linux")]
        println!("Process names are case sensitive and match exactly.");
    }
    println!(
        "HTTPS mode: {}.",
        if decrypt {
            "Decrypt selected HTTPS connections using the interception CA"
        } else {
            "Opaque CONNECT tunnels; HTTPS capture contains destination and byte summaries, with no interception CA. Selected plain HTTP remains inspectable"
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_and_filters_are_run_scoped() {
        assert_eq!(
            process_policy(&[]).unwrap().mode,
            ProcessInspectionMode::All
        );
        #[cfg(any(windows, target_os = "linux"))]
        {
            let policy = process_policy(&[
                "--process".into(),
                " browser ".into(),
                "--process".into(),
                "worker".into(),
            ])
            .unwrap();
            assert_eq!(policy.mode, ProcessInspectionMode::Only);
            #[cfg(windows)]
            assert_eq!(policy.include, ["browser.exe", "worker.exe"]);
            #[cfg(not(windows))]
            assert_eq!(policy.include, ["browser", "worker"]);
            assert_eq!(
                process_policy(&["--exclude-process".into(), "worker".into()])
                    .unwrap()
                    .mode,
                ProcessInspectionMode::Except
            );
        }
        assert_eq!(
            process_policy(&[]).unwrap().mode,
            ProcessInspectionMode::All
        );
    }

    #[test]
    fn conflicting_and_empty_process_policies_fail() {
        for args in [
            vec!["--process", "browser", "--exclude-process", "worker"],
            vec!["--process", "   "],
            vec!["--exclude-process", "  "],
        ] {
            assert!(
                process_policy(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
    }

    #[test]
    fn values_that_look_like_process_options_do_not_select_processes() {
        let arguments = ["--password", "--process"].map(str::to_owned);
        assert_eq!(
            process_policy(&arguments).unwrap().mode,
            ProcessInspectionMode::All
        );
    }

    #[test]
    fn password_values_do_not_disable_https_decryption() {
        assert!(decrypt_https(&[
            "--password".into(),
            "--no-decrypt-https".into()
        ]));
        assert!(!decrypt_https(&[
            "--password".into(),
            "--no-decrypt-https".into(),
            "--no-decrypt-https".into()
        ]));
    }
}
