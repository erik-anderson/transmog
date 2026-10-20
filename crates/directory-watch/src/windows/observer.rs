use super::{Command, OwnedHandle, io, mpsc, native::Entry};
use std::{collections::BTreeMap, os::windows::io::AsRawHandle as _};
use windows_sys::Win32::{
    Foundation::{ERROR_NOTIFY_ENUM_DIR, GetLastError},
    System::{IO::GetQueuedCompletionStatus, Threading::INFINITE},
};
const MAX_WATCHES: usize = 32;

pub(super) fn run(port: &OwnedHandle, commands: &mpsc::Receiver<Command>) {
    let mut entries = BTreeMap::<usize, Entry>::new();
    let mut next_id = 1_usize;
    loop {
        let (mut bytes, mut id, mut overlapped) = (0, 0, std::ptr::null_mut());
        // All issuance and completion run on this persistent thread. Command wakes
        // have key zero and null OVERLAPPED; real entries have non-reused positive IDs.
        let success = unsafe {
            GetQueuedCompletionStatus(
                port.as_raw_handle(),
                &raw mut bytes,
                &raw mut id,
                &raw mut overlapped,
                INFINITE,
            )
        };
        let error = if success == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        if overlapped.is_null() {
            if success == 0 {
                break;
            }
            for command in commands.try_iter() {
                if !dispatch(command, port, &mut entries, &mut next_id) {
                    return;
                }
            }
            continue;
        }
        let Some(entry) = entries.get_mut(&id) else {
            continue;
        };
        if !std::ptr::eq(overlapped, &raw const *entry.overlapped) {
            break;
        }
        entry.pending = false;
        if let Some(ready) = entry.closing.take() {
            // Complete directory close before acknowledging removal to its owner.
            entries.remove(&id);
            let _ = ready.send(());
            continue;
        }
        complete(entry, bytes, error);
    }
    // Entry drop joins any still-pending kernel access before buffers are freed.
}
fn dispatch(
    command: Command,
    port: &OwnedHandle,
    entries: &mut BTreeMap<usize, Entry>,
    next_id: &mut usize,
) -> bool {
    match command {
        Command::Add { path, queue, ready } => {
            let result = if entries.len() >= MAX_WATCHES || *next_id == usize::MAX {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Close another developer visualizer before watching more sources",
                ))
            } else {
                let id = *next_id;
                *next_id += 1;
                Entry::open(&path, queue, port, id).map(|entry| {
                    entries.insert(id, entry);
                    id
                })
            };
            let _ = ready.send(result);
        }
        Command::Resume(id) => {
            if let Some(entry) = entries.get_mut(&id)
                && !entry.pending
                && entry.closing.is_none()
                && entry.queue.lock().is_ok_and(|queue| queue.failed.is_none())
                && let Err(error) = entry.arm()
            {
                fail(entry, error);
            }
        }
        Command::Remove(id, ready) => {
            if let Some(entry) = entries.get_mut(&id)
                && entry.pending
            {
                entry.closing = Some(ready);
                entry.cancel();
            } else {
                entries.remove(&id);
                let _ = ready.send(());
            }
        }
        Command::Stop => return false,
    }
    true
}
fn complete(entry: &mut Entry, bytes: u32, error: u32) {
    if error != 0 && error != ERROR_NOTIFY_ENUM_DIR {
        fail(entry, io::Error::from_raw_os_error(error.cast_signed()));
        return;
    }
    let changes = entry.changes(if error == ERROR_NOTIFY_ENUM_DIR {
        0
    } else {
        bytes
    });
    // Copy/validate before rearming. Rearm immediately for the first batch, so an
    // ordinary save never waits for UI work. A second unconsumed batch pauses this
    // watcher only, bounding memory and CPU during storms or slow visualizations.
    let rearm = entry
        .queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(changes);
    if rearm && let Err(error) = entry.arm() {
        fail(entry, error);
    }
}
fn fail(entry: &Entry, error: io::Error) {
    entry
        .queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .failed = Some(error);
}
