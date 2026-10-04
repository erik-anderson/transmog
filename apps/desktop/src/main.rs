//! Native entry point for the rustymiddle desktop shell.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    rustymiddle_desktop::run();
}
