//! Advisory file locks (`flock`). One per workspace file, one for the watcher.

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Held until dropped.
pub struct FileLock {
    file: File,
}

impl FileLock {
    fn open(path: &Path) -> std::io::Result<File> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
    }

    /// Block until the lock is ours.
    pub fn acquire(path: &Path) -> std::io::Result<FileLock> {
        let file = Self::open(path)?;
        loop {
            // SAFETY: flock on a valid, owned descriptor.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc == 0 {
                return Ok(FileLock { file });
            }
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }

    /// Take the lock if nobody holds it.
    pub fn try_acquire(path: &Path) -> std::io::Result<Option<FileLock>> {
        let file = Self::open(path)?;
        // SAFETY: flock on a valid, owned descriptor.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(Some(FileLock { file }));
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(None)
        } else {
            Err(err)
        }
    }

    pub fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // SAFETY: flock on a valid, owned descriptor.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Checking a lock takes it for a moment, so a single failed try may be
/// another checker. Held means held on every try over about 100 ms.
const TRIES: u32 = 5;
const TRY_GAP: std::time::Duration = std::time::Duration::from_millis(20);

/// Is the lock held by a long-lived owner (not just another checker)?
pub fn is_held(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    for i in 0..TRIES {
        if !matches!(FileLock::try_acquire(path), Ok(None)) {
            return false;
        }
        if i + 1 < TRIES {
            std::thread::sleep(TRY_GAP);
        }
    }
    true
}

/// Take the lock unless a long-lived owner has it; a checker holding it for
/// a moment does not count.
pub fn acquire_unless_owned(path: &Path) -> std::io::Result<Option<FileLock>> {
    for i in 0..TRIES * 5 {
        if let Some(lock) = FileLock::try_acquire(path)? {
            return Ok(Some(lock));
        }
        if i + 1 < TRIES * 5 {
            std::thread::sleep(TRY_GAP);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_try_fails_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let first = FileLock::try_acquire(&path).unwrap();
        assert!(first.is_some());
        assert!(FileLock::try_acquire(&path).unwrap().is_none());
        assert!(is_held(&path));
        drop(first);
        assert!(FileLock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn a_brief_holder_is_not_an_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let brief = FileLock::try_acquire(&path).unwrap().unwrap();
        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || acquire_unless_owned(&path).unwrap().is_some())
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        drop(brief);
        assert!(
            waiter.join().unwrap(),
            "gave up on a lock held only briefly"
        );
        assert!(!is_held(&path));
    }
}
