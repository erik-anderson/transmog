//! Native entry point for the Transmog desktop shell.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    transmog_desktop::run();
}
