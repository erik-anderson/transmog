//! Transient, masked capture passwords; never accepted as command-line text.
use std::{
    fs::File,
    io::{self, IsTerminal, Read},
};
use transmog_capture::CapturePassword;
use zeroize::Zeroizing;

pub(crate) fn read_file(path: &str) -> io::Result<CapturePassword> {
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("Password file exceeds 4096 bytes"));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::other("Password file must contain UTF-8 text"))?
        .trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return Err(io::Error::other("Password cannot be empty"));
    }
    Ok(CapturePassword::new(text.to_owned()))
}
pub(crate) fn prompt(confirm: bool) -> io::Result<CapturePassword> {
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(
            "A capture password is needed. Use an interactive console or a protected --password-file.",
        ));
    }
    loop {
        let password = Zeroizing::new(rpassword::prompt_password("Capture password: ")?);
        if password.len() > 4096 {
            return Err(io::Error::other(
                "Password must contain at most 4096 UTF-8 bytes",
            ));
        }
        if password.is_empty() {
            return Err(io::Error::other("Password entry canceled"));
        }
        if confirm {
            let repeated = Zeroizing::new(rpassword::prompt_password("Confirm password: ")?);
            if *password != *repeated {
                eprintln!("Passwords do not match. Try again, or press Ctrl+C to cancel.");
                continue;
            }
            println!("Keep this password to reopen the trace. Transmog cannot recover it.");
        }
        return Ok(CapturePassword::new(password.to_string()));
    }
}
pub(crate) fn output(arguments: &[String]) -> io::Result<Option<CapturePassword>> {
    if !arguments.iter().any(|arg| arg == "--encrypt") {
        return Ok(None);
    }
    crate::option(arguments, "--password-file")
        .map_or_else(|| prompt(true).map(Some), |path| read_file(path).map(Some))
}
