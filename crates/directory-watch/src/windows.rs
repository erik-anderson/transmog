use super::{Notifications, Path, io};
use std::{
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    sync::{Arc, Mutex, OnceLock, Weak, mpsc},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    System::IO::{CreateIoCompletionPort, PostQueuedCompletionStatus},
};
mod native;
mod observer;
#[cfg(test)]
mod tests;

#[derive(Default)]
struct Queue {
    pending: Option<Notifications>,
    failed: Option<io::Error>,
}
impl Queue {
    fn push(&mut self, changes: Notifications) -> bool {
        if let Some(pending) = &mut self.pending {
            // Merge a second completion without manufacturing loss for unrelated
            // files. Pause rearming until consumption, with hard name/byte ceilings.
            pending.overflowed |= changes.overflowed;
            let mut names = std::collections::BTreeSet::new();
            let mut bytes = 0;
            for name in std::mem::take(&mut pending.names)
                .into_iter()
                .chain(changes.names)
            {
                if names.contains(&name) {
                    continue;
                }
                if names.len() == 1024 || bytes + name.len() > 32 * 1024 {
                    pending.overflowed = true;
                } else {
                    bytes += name.len();
                    names.insert(name);
                }
            }
            pending.names = names.into_iter().collect();
            false
        } else {
            self.pending = Some(changes);
            true
        }
    }
}
type SharedQueue = Arc<Mutex<Queue>>;
enum Command {
    Add {
        path: std::path::PathBuf,
        queue: SharedQueue,
        ready: mpsc::SyncSender<io::Result<usize>>,
    },
    Resume(usize),
    Remove(usize, mpsc::SyncSender<()>),
    Stop,
}
struct Observer {
    port: OwnedHandle,
    commands: mpsc::Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Observer {
    fn get() -> io::Result<Arc<Self>> {
        static SHARED: OnceLock<Mutex<Weak<Observer>>> = OnceLock::new();
        let mut shared = SHARED
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| io::Error::other("Source observer is unavailable"))?;
        if let Some(observer) = shared.upgrade() {
            return Ok(observer);
        }
        // A process-owned port, not inherited or exposed to extension workers.
        let port =
            unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) };
        if port.is_null() {
            return Err(io::Error::last_os_error());
        }
        // The successful native call transferred this unique handle to its owner.
        let port = unsafe { OwnedHandle::from_raw_handle(port) };
        let thread_port = port.try_clone()?;
        let (commands, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("transmog-source-watch".into())
            .stack_size(256 * 1024)
            .spawn(move || observer::run(&thread_port, &receiver))?;
        let observer = Arc::new(Self {
            port,
            commands,
            thread: Some(thread),
        });
        *shared = Arc::downgrade(&observer);
        Ok(observer)
    }
    fn send(&self, command: Command) -> io::Result<()> {
        self.commands
            .send(command)
            .map_err(|_| io::Error::other("Source observer ended"))?;
        // Key zero is reserved for commands; no OVERLAPPED pointer is supplied.
        if unsafe { PostQueuedCompletionStatus(self.port.as_raw_handle(), 0, 0, std::ptr::null()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        let _ = self.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// An active directory watch. All watches share one sleeping metadata I/O thread,
/// which is joined after the last watch is dropped. No file contents are read.
pub struct DirectoryWatch {
    observer: Arc<Observer>,
    id: usize,
    queue: SharedQueue,
}
impl DirectoryWatch {
    /// Watches a non-reparse directory. Pin security-sensitive ancestors separately.
    /// # Errors
    /// Returns capacity, filesystem, notification or observer initialization errors.
    pub fn open(path: &Path) -> io::Result<Self> {
        let observer = Observer::get()?;
        let queue = Arc::new(Mutex::new(Queue::default()));
        let (ready, receiver) = mpsc::sync_channel(1);
        observer.send(Command::Add {
            path: path.to_path_buf(),
            queue: Arc::clone(&queue),
            ready,
        })?;
        let id = receiver
            .recv()
            .map_err(|_| io::Error::other("Source watch initialization ended"))??;
        Ok(Self {
            observer,
            id,
            queue,
        })
    }
    /// Takes one bounded metadata batch without waiting or opening source files.
    /// # Errors
    /// Returns a notification error or an unexpected observer exit.
    pub fn poll(&mut self) -> io::Result<Option<Notifications>> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| io::Error::other("Source notifications are unavailable"))?;
        if let Some(error) = queue.failed.take() {
            return Err(error);
        }
        if self
            .observer
            .thread
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            return Err(io::Error::other("Source observer ended"));
        }
        let result = queue.pending.take();
        drop(queue);
        if result.is_some() {
            self.observer.send(Command::Resume(self.id))?;
        }
        Ok(result)
    }
}
impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        let (ready, receiver) = mpsc::sync_channel(1);
        if self.observer.send(Command::Remove(self.id, ready)).is_ok() {
            // Removal acknowledges only after the exact completion packet is dequeued
            // and the directory handle is closed. No kernel-owned pointer outlives it.
            let _ = receiver.recv();
        }
    }
}
