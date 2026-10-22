use super::{Notifications, Path, SharedQueue, io, mpsc};
use std::{
    fs::{File, OpenOptions},
    os::windows::{
        fs::{MetadataExt as _, OpenOptionsExt as _},
        io::{AsRawHandle as _, OwnedHandle},
    },
};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, GetLastError},
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
    },
    System::IO::{CancelIoEx, CreateIoCompletionPort, GetOverlappedResult, OVERLAPPED},
};
pub(super) const BUFFER_BYTES: usize = 16 * 1024;
/// Constructed, armed, completed and destroyed only on the shared observer thread.
pub(super) struct Entry {
    file: File,
    pub overlapped: Box<OVERLAPPED>,
    buffer: Box<[u32]>,
    pub queue: SharedQueue,
    pub pending: bool,
    pub closing: Option<mpsc::SyncSender<()>>,
}
impl Entry {
    pub fn open(
        path: &Path,
        queue: SharedQueue,
        port: &OwnedHandle,
        id: usize,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY)
            // Deliberately pin this root against replacement, alongside the caller's
            // non-reparse ancestor handles. Child file atomic saves remain possible.
            .share_mode(3)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OVERLAPPED,
            )
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A source watch requires a non-reparse directory",
            ));
        }
        // Association does not transfer file ownership. The ID never denotes a pointer.
        if unsafe { CreateIoCompletionPort(file.as_raw_handle(), port.as_raw_handle(), id, 1) }
            .is_null()
        {
            return Err(io::Error::last_os_error());
        }
        let mut entry = Self {
            file,
            overlapped: Box::default(),
            buffer: vec![0; BUFFER_BYTES / 4].into_boxed_slice(),
            queue,
            pending: false,
            closing: None,
        };
        entry.arm()?;
        Ok(entry)
    }
    pub fn arm(&mut self) -> io::Result<()> {
        debug_assert!(!self.pending);
        *self.overlapped = OVERLAPPED::default();
        // Stable, DWORD-aligned boxes remain owned until the exact completion is
        // dequeued. There is at most one outstanding operation per directory.
        let success = unsafe {
            ReadDirectoryChangesW(
                self.file.as_raw_handle(),
                self.buffer.as_mut_ptr().cast(),
                u32::try_from(BUFFER_BYTES).unwrap(),
                1,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
                std::ptr::null_mut(),
                &raw mut *self.overlapped,
                None,
            )
        };
        if success == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        self.pending = true;
        Ok(())
    }
    pub fn cancel(&self) {
        // Cancel only this owned operation. ERROR_NOT_FOUND can mean its completion
        // is already queued; the entry still survives until that packet is consumed.
        unsafe {
            CancelIoEx(self.file.as_raw_handle(), &raw const *self.overlapped);
        }
    }
    pub fn changes(&self, bytes: u32) -> Notifications {
        if bytes as usize > BUFFER_BYTES {
            return parse(&[]);
        }
        // Only called after completion, before rearming; the kernel cannot modify
        // these initialized bytes. The extent has been checked against the allocation.
        parse(unsafe { std::slice::from_raw_parts(self.buffer.as_ptr().cast(), bytes as usize) })
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        if self.pending {
            // Defensive unwind path: join kernel access even if the observer panics.
            // Normally removal retains this entry until IOCP delivers cancellation.
            self.cancel();
            let mut bytes = 0;
            unsafe {
                GetOverlappedResult(
                    self.file.as_raw_handle(),
                    &raw const *self.overlapped,
                    &raw mut bytes,
                    1,
                );
            }
        }
    }
}
pub(super) fn parse(bytes: &[u8]) -> Notifications {
    let mut result = Notifications {
        names: Vec::new(),
        overflowed: bytes.is_empty(),
    };
    if bytes.is_empty() {
        return result;
    }
    let mut offset = 0;
    loop {
        let Some(header) = bytes.get(offset..offset + 12) else {
            result.overflowed = true;
            break;
        };
        let next = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
        if length > bytes.len() - offset - 12 || !length.is_multiple_of(2) {
            result.overflowed = true;
            break;
        }
        let name: Vec<u16> = bytes[offset + 12..offset + 12 + length]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        match String::from_utf16(&name) {
            Ok(name) if !name.is_empty() => result.names.push(name),
            _ => result.overflowed = true,
        }
        if next == 0 {
            break;
        }
        if !next.is_multiple_of(4) || next < 12 + length || next > bytes.len() - offset {
            result.overflowed = true;
            break;
        }
        offset += next;
    }
    result
}
