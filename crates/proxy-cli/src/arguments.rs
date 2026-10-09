//! Command option validation before any file, certificate, or proxy side effect.

use std::{collections::HashSet, io};

pub(crate) fn validate(arguments: &[String], values: &[&str], flags: &[&str]) -> io::Result<()> {
    let mut seen = HashSet::new();
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        if !values.contains(&argument.as_str()) && !flags.contains(&argument.as_str()) {
            return Err(crate::invalid_input(format!("Unknown option {argument}")));
        }
        if !seen.insert(argument) {
            return Err(crate::invalid_input(format!("Duplicate option {argument}")));
        }
        index += 1;
        if values.contains(&argument.as_str()) {
            if arguments
                .get(index)
                .is_none_or(|value| value.is_empty() || value.starts_with("--"))
            {
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
