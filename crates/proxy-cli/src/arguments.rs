//! Command option validation before any file, certificate, or proxy side effect.

use std::{collections::HashSet, io};

/// Walk option positions, consuming values even when the value resembles an option.
/// Command validation still defines which options and repetitions it accepts.
pub(crate) fn options(arguments: &[String]) -> impl Iterator<Item = (&str, Option<&str>)> {
    let mut index = 0;
    std::iter::from_fn(move || {
        let argument = arguments.get(index)?;
        index += 1;
        let value = if takes_value(argument) {
            let value = arguments.get(index).map(String::as_str);
            index += 1;
            value
        } else {
            None
        };
        Some((argument.as_str(), value))
    })
}

pub(crate) fn has_flag(arguments: &[String], name: &str) -> bool {
    options(arguments).any(|(option, value)| option == name && value.is_none())
}

pub(crate) fn takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "--profile"
            | "--duration"
            | "--client-version"
            | "--expected"
            | "--actual"
            | "--output"
            | "--listen"
            | "--route"
            | "--upstream-ca-cert"
            | "--request-body-limit"
            | "--password-file"
            | "--password"
            | "--id"
            | "--source-password-file"
            | "--circular-buffer"
            | "--ca-cert"
            | "--ca-key"
            | "--proof-id"
            | "--capture"
            | "--cert"
            | "--key"
            | "--name"
            | "--identity"
            | "--days"
            | "--input"
            | "--format"
            | "--process"
            | "--exclude-process"
    )
}

pub(crate) fn validate(arguments: &[String], values: &[&str], flags: &[&str]) -> io::Result<()> {
    validate_repeatable(arguments, values, flags, &[])
}

pub(crate) fn validate_repeatable(
    arguments: &[String],
    values: &[&str],
    flags: &[&str],
    repeatable: &[&str],
) -> io::Result<()> {
    let mut seen = HashSet::new();
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let takes_value =
            values.contains(&argument.as_str()) || repeatable.contains(&argument.as_str());
        if !takes_value && !flags.contains(&argument.as_str()) {
            return Err(crate::invalid_input(format!("Unknown option {argument}")));
        }
        if !seen.insert(argument) && !repeatable.contains(&argument.as_str()) {
            return Err(crate::invalid_input(format!("Duplicate option {argument}")));
        }
        index += 1;
        if takes_value {
            if arguments.get(index).is_none_or(|value| {
                value.is_empty() || (argument != "--password" && value.starts_with("--"))
            }) {
                return Err(crate::invalid_input(format!("{argument} requires a value")));
            }
            index += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicitly_repeatable_values_may_repeat() {
        let valid = [
            "--process",
            "browser",
            "--process",
            "worker",
            "--output",
            "trace",
        ]
        .map(str::to_owned);
        assert!(validate_repeatable(&valid, &["--output"], &[], &["--process"]).is_ok());
        for invalid in [
            vec!["--process", "browser", "--process"],
            vec!["--process", "browser", "--process", "--output", "trace"],
            vec!["--process", "browser", "--output", "one", "--output", "two"],
        ] {
            assert!(
                validate_repeatable(
                    &invalid.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    &["--output"],
                    &[],
                    &["--process"]
                )
                .is_err()
            );
        }
    }

    #[test]
    fn option_values_cannot_become_option_positions() {
        let arguments = ["--password", "--process", "--process", "browser"].map(str::to_owned);
        assert_eq!(
            options(&arguments).collect::<Vec<_>>(),
            [
                ("--password", Some("--process")),
                ("--process", Some("browser"))
            ]
        );
        for flag in ["--no-decrypt-https", "--install-root", "--persistent-root"] {
            let arguments = ["--password", flag].map(str::to_owned);
            assert!(!has_flag(&arguments, flag));
            let arguments = ["--password", flag, flag].map(str::to_owned);
            assert!(has_flag(&arguments, flag));
        }
    }

    #[test]
    fn values_flags_and_stdout_are_unambiguous() {
        let arguments = ["--output", "-", "--encrypt"].map(str::to_owned);
        assert!(validate(&arguments, &["--output"], &["--encrypt"]).is_ok());
        for arguments in [
            vec!["--output"],
            vec!["--output", "--encrypt"],
            vec!["--output", ""],
            vec!["--encrypton"],
            vec!["--output", "first", "--output", "second"],
            vec!["--encrypt", "--encrypt"],
            vec!["unexpected-positional-value"],
        ] {
            assert!(
                validate(
                    &arguments.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    &["--output"],
                    &["--encrypt"]
                )
                .is_err()
            );
        }
    }
}
