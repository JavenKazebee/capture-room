//! Rotating the service's log file. launchd opens the file once and hands it
//! to the node as stdout and stderr, so it would grow forever. Past
//! [`MAX_BYTES`] the node renames it to `.1` (shifting older ones up to
//! [`KEEP`]) and points stdout and stderr at a fresh file, which catches
//! GStreamer's and the SDKs' own output as well as `tracing`'s.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Old files kept beside the current one.
const KEEP: u32 = 4;
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// Check the log file now and every minute, while the node runs as the
/// service.
pub fn spawn() {
    let Some(path) = super::log_file() else {
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CHECK_EVERY);
        loop {
            tick.tick().await;
            if let Err(e) = rotate_if_full(&path) {
                tracing::warn!(error = %e, path = %path.display(), "couldn't rotate the log file");
            }
        }
    });
}

fn rotate_if_full(path: &Path) -> io::Result<()> {
    if std::fs::metadata(path)?.len() < MAX_BYTES {
        return Ok(());
    }
    let file = rotate(path, KEEP)?;
    // Atomic: a write in between goes to the old file, now `.1`.
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if unsafe { libc::dup2(file.as_raw_fd(), fd) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    tracing::info!(path = %path.display(), "rotated the log file");
    Ok(())
}

/// Shift `path` to `path.1`, `.1` to `.2` and so on, dropping what's past
/// `keep`, and open a new, empty `path` for appending.
fn rotate(path: &Path, keep: u32) -> io::Result<File> {
    for n in (1..keep).rev() {
        match std::fs::rename(numbered(path, n), numbered(path, n + 1)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    std::fs::rename(path, numbered(path, 1))?;
    OpenOptions::new().create(true).append(true).open(path)
}

fn numbered(path: &Path, n: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_shifts_and_drops_the_oldest() {
        let dir = std::env::temp_dir().join(format!("log-rotation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("node.log");
        let read = |n: u32| std::fs::read_to_string(numbered(&log, n)).ok();

        for text in ["a", "b", "c", "d"] {
            std::fs::write(&log, text).unwrap();
            drop(rotate(&log, 3).unwrap());
        }

        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
        assert_eq!(read(1).as_deref(), Some("d"));
        assert_eq!(read(2).as_deref(), Some("c"));
        assert_eq!(read(3).as_deref(), Some("b"));
        assert_eq!(read(4), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
