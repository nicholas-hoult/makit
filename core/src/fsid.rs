//! Identity of the file behind a path (platform matrix P6): lets the readers notice that a file was replaced
//! (atomic rename, delete + recreate) and must be read from the start, instead of trusting an old offset.
//!
//! unix: the inode. Windows: the NTFS file index. The creation time is NOT usable there: NTFS "file tunneling"
//! gives a file recreated at the same name within seconds its old creation time back.

use std::fs::File;
use std::path::Path;

#[cfg(unix)]
pub fn of_file(f: &File) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    f.metadata().ok().map(|m| m.ino())
}

#[cfg(windows)]
pub fn of_file(f: &File) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    // SAFETY: plain out-parameter struct, the handle stays valid for the duration of the call
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return None;
    }
    Some(((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64)
}

#[cfg(not(any(unix, windows)))]
pub fn of_file(_f: &File) -> Option<u64> {
    None
}

pub fn of_path(p: &Path) -> Option<u64> {
    File::open(p).ok().and_then(|f| of_file(&f))
}
