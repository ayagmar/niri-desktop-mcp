//! The lease: one server at a time controls a niri instance. It is an exclusive
//! non-blocking `flock` on `<runtime dir>/lease`, a file that is created once and never
//! removed, so every server locks the same inode. The kernel drops the lock when the
//! holder's file closes, including when the process dies. `lease.json` beside it names
//! the holder for other servers to display; it is cleared on release, and a holder whose
//! process is gone is ignored.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::runtime::RuntimeDir;

const LEASE: &str = "lease";
const HOLDER: &str = "lease.json";

/// Who holds the lease, as written to `lease.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Holder {
    pub(crate) pid: u32,
    /// The MCP client's name and the server's PID, such as `claude-code/4711`.
    pub(crate) label: String,
    pub(crate) since: String,
}

/// A held lease. Dropping it clears `lease.json` and unlocks.
#[derive(Debug)]
pub(crate) struct Lease {
    file: File,
    holder: Holder,
    locked: Locked,
    record: std::path::PathBuf,
}

/// The file a lease locked, which can be checked without the lease itself.
#[derive(Debug, Clone)]
pub(crate) struct Locked {
    path: std::path::PathBuf,
    dev: u64,
    ino: u64,
}

impl Locked {
    /// Whether the lease's path still names the locked file. If it was removed or
    /// replaced, another server could lock the new file, so the lease no longer excludes
    /// anyone.
    pub(crate) fn intact(&self) -> bool {
        std::fs::metadata(&self.path)
            .is_ok_and(|named| named.dev() == self.dev && named.ino() == self.ino)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Another process holds the lock; its record, if it left a readable one.
    Held(Option<Holder>),
    Io(String),
}

impl Lease {
    /// Takes the lease without waiting, creating the runtime directory as needed.
    pub(crate) fn acquire(runtime: &RuntimeDir, label: &str) -> Result<Self, Refused> {
        let io = |action: &str, path: &Path, error: io::Error| {
            Refused::Io(format!("{action} {}: {error}", path.display()))
        };
        runtime
            .create()
            .map_err(|error| io("create", runtime.path(), error))?;
        let path = runtime.path().join(LEASE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| io("open", &path, error))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(Refused::Held(holder(runtime))),
            Err(TryLockError::Error(error)) => return Err(io("lock", &path, error)),
        }
        let opened = file.metadata().map_err(|error| io("stat", &path, error))?;
        let locked = Locked {
            dev: opened.dev(),
            ino: opened.ino(),
            path,
        };
        let holder = Holder {
            pid: std::process::id(),
            label: label.to_owned(),
            since: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        };
        let record = runtime.path().join(HOLDER);
        write_record(&record, Some(&holder)).map_err(|error| io("write", &record, error))?;
        Ok(Self {
            file,
            holder,
            locked,
            record,
        })
    }

    pub(crate) const fn holder(&self) -> &Holder {
        &self.holder
    }

    /// Whether `lease` still names the locked file; see `Locked::intact`.
    pub(crate) fn intact(&self) -> bool {
        self.locked.intact()
    }

    pub(crate) const fn locked(&self) -> &Locked {
        &self.locked
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Clear the record while still holding the lock, so no new holder's record is
        // erased. If `lease` was replaced, another server may hold the new file and own the
        // record now, so leave it. Unlocking can't fail in a way that matters: closing the
        // file unlocks.
        if self.intact() {
            write_record(&self.record, None).ok();
        }
        self.file.unlock().ok();
    }
}

/// The current holder, from `lease.json`, if its process is still running.
pub(crate) fn holder(runtime: &RuntimeDir) -> Option<Holder> {
    let text = std::fs::read_to_string(runtime.path().join(HOLDER)).ok()?;
    let holder: Holder = serde_json::from_str(&text).ok()?;
    Path::new(&format!("/proc/{}", holder.pid))
        .exists()
        .then_some(holder)
}

/// Writes the holder, or empties the file for `None`.
fn write_record(path: &Path, holder: Option<&Holder>) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    if let Some(holder) = holder {
        file.write_all(&serde_json::to_vec(holder)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(dir: &Path) -> RuntimeDir {
        RuntimeDir::of(&crate::test_support::niri_env(dir)).unwrap()
    }

    #[test]
    fn one_holder_at_a_time_named_in_the_record() {
        let dir = crate::test_support::fresh_dir("lease");
        let runtime = runtime(&dir);
        assert_eq!(holder(&runtime), None);
        let first = Lease::acquire(&runtime, "first/1").unwrap();
        assert_eq!(first.holder().label, "first/1");
        assert_eq!(first.holder().pid, std::process::id());
        assert_eq!(holder(&runtime).as_ref(), Some(first.holder()));
        // A second open file description can't take the lock, even in this process.
        assert_eq!(
            Lease::acquire(&runtime, "second/2").unwrap_err(),
            Refused::Held(Some(first.holder().clone()))
        );
        assert!(first.intact());
        drop(first);
        assert_eq!(holder(&runtime), None);
        let second = Lease::acquire(&runtime, "second/2").unwrap();
        assert_eq!(holder(&runtime).unwrap().label, "second/2");
        std::fs::remove_file(runtime.path().join(LEASE)).unwrap();
        assert!(!second.intact());
        // A third server takes the new file; the second's cleanup leaves its record alone.
        let third = Lease::acquire(&runtime, "third/3").unwrap();
        drop(second);
        assert_eq!(holder(&runtime).as_ref(), Some(third.holder()));
        drop(third);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_record_left_by_a_dead_process_names_nobody() {
        let dir = crate::test_support::fresh_dir("lease-stale");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        let dead = Holder {
            pid: u32::MAX,
            label: "gone/1".to_owned(),
            since: "2026-10-08T00:00:00.000Z".to_owned(),
        };
        write_record(&runtime.path().join(HOLDER), Some(&dead)).unwrap();
        assert_eq!(holder(&runtime), None);
        // The lock itself is free, so the lease can be taken.
        drop(Lease::acquire(&runtime, "next/2").unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
