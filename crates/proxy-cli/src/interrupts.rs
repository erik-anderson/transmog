//! Retain one console interrupt subscription from startup through shutdown.

use std::io;

#[cfg(windows)]
pub(crate) use tokio::signal::windows::{CtrlC as Interrupts, ctrl_c as register};

#[cfg(unix)]
pub(crate) use tokio::signal::unix::Signal as Interrupts;

#[cfg(unix)]
pub(crate) fn register() -> io::Result<Interrupts> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
}

pub(crate) async fn wait(interrupts: &mut Interrupts) -> io::Result<()> {
    interrupts
        .recv()
        .await
        .ok_or_else(|| io::Error::other("The Ctrl+C listener closed unexpectedly"))
}
