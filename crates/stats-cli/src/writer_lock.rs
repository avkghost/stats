//! Advisory writer lock shared by every command that changes the store.
//!
//! The lock is taken once per command by the caller, never inside the
//! functions that do the work. Taking it twice in one process is refused by
//! the operating system, because `flock` locks belong to the open file
//! description and a second open conflicts with the first.

use anyhow::Context;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

/// Name of the lock file inside the data directory.
pub const LOCK_FILE_NAME: &str = "stats.lock";

#[cfg(test)]
pub(crate) static ACQUISITIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Holds the writer lock for as long as it is alive. Released on drop.
#[derive(Debug)]
pub struct WriterLock {
    _file: File,
}

impl WriterLock {
    /// Acquires the exclusive lock without waiting. Fails at once when another
    /// process holds it, so a refused command writes nothing.
    pub fn acquire(data_dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(data_dir)
            .with_context(|| format!("create data directory {}", data_dir.display()))?;
        let path: PathBuf = data_dir.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        lock_exclusive_nonblocking(&file, &path)?;
        #[cfg(test)]
        ACQUISITIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Self { _file: file })
    }
}

#[cfg(unix)]
fn lock_exclusive_nonblocking(file: &File, path: &Path) -> anyhow::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `file` is open for the duration of the call, so its descriptor is
    // valid. `flock` reads no memory from us.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        anyhow::bail!(
            "another stats process holds {}; wait for it to finish and retry",
            path.display()
        );
    }
    Err(error).with_context(|| format!("lock {}", path.display()))
}

#[cfg(not(unix))]
fn lock_exclusive_nonblocking(_file: &File, _path: &Path) -> anyhow::Result<()> {
    // Advisory inode locks are not available here; the writer lock is a no-op.
    // Concurrent writers on these platforms are not protected (see the design).
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "stats-cli-writer-lock-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    #[cfg(unix)]
    #[test]
    fn second_acquisition_is_refused_while_first_is_held() {
        let directory = temp_data_dir("held");
        let first = WriterLock::acquire(&directory).unwrap();
        let error = WriterLock::acquire(&directory).unwrap_err();
        assert!(
            error.to_string().contains("another stats process holds"),
            "{error}"
        );
        drop(first);
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn lock_is_released_on_drop() {
        let directory = temp_data_dir("released");
        drop(WriterLock::acquire(&directory).unwrap());
        let _again = WriterLock::acquire(&directory).unwrap();
        let _ = fs::remove_dir_all(directory);
    }
}
