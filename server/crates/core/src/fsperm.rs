//! Owner-only files and directories for the data dir (#14).
//!
//! Secrets (`signing.key`) and the database (`reel.db` and its `-wal`/`-shm`) are created with their final
//! mode in the `open`/`mkdir` call itself, so they are never readable by others, not even briefly. A mode
//! passed to `open`/`mkdir` can only be narrowed by the umask, never widened. Startup also repairs files and
//! directories left with looser modes by older builds.
//!
//! The data dir itself is `0700`, which is what protects the caches inside it (images, transcode segments,
//! subtitles, keyframe indexes): they keep the default umask, but nobody else can traverse to them.
//!
//! On non-Unix platforms these functions only create files and directories; modes don't exist there.

use std::{io, path::Path};

/// Mode of private directories.
pub const DIR_MODE: u32 = 0o700;
/// Mode of private files.
pub const FILE_MODE: u32 = 0o600;

/// Create `path` and any missing parents as `0700`. Existing directories are left alone (see
/// [`ensure_private_dir`] to also repair them).
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(DIR_MODE);
    }
    b.create(path)
}

/// Create the directory `path` (`0700`) or, if it exists, take away group/other access. A failed repair is
/// logged, not returned: a data dir owned by someone else should not stop the server.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    create_private_dir_all(path)?;
    repair(path);
    Ok(())
}

/// Take away group/other permission bits from an existing file or directory (owner bits are kept, so `0644`
/// becomes `0600` and `0755` becomes `0700`). A missing path is fine. Failures are logged; returns whether the
/// mode was changed.
pub fn repair(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
            Err(e) => {
                tracing::warn!("cannot check permissions of {}: {e}", path.display());
                return false;
            }
        };
        let old = meta.permissions().mode() & 0o7777;
        if old & 0o077 == 0 {
            return false;
        }
        let new = old & !0o077;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(new)) {
            Ok(()) => {
                tracing::info!(
                    "tightened permissions of {} from {old:04o} to {new:04o}",
                    path.display()
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    "{} is {old:04o} (readable by other users) and cannot be tightened: {e}",
                    path.display()
                );
                false
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Create an empty `0600` file at `path` unless something already exists there (which is left untouched).
pub fn create_private_file(path: &Path) -> io::Result<()> {
    match private_options().open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Atomically replace `path` with `contents` as a `0600` file: the bytes go to a new `0600` temp file in the
/// same directory, which is synced and then renamed over `path`. Readers see the old file or the new one,
/// never a partial or readable-by-others one. The parent directory must exist.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut last = None;
    for attempt in 0..16u32 {
        let mut tmp_name = std::ffi::OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".{}.{attempt}.tmp", std::process::id()));
        let tmp = dir.join(tmp_name);
        let mut f = match private_options().open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        let res = f
            .write_all(contents)
            .and_then(|()| f.sync_all())
            .and_then(|()| {
                drop(f);
                std::fs::rename(&tmp, path)
            });
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return res;
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free temp file name")))
}

/// `create_new` + mode `0600`: the file is born private (no write-then-chmod window).
fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(FILE_MODE);
    }
    o
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, m: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }

    #[test]
    fn new_dirs_are_0700_including_parents() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("a/b/c");
        ensure_private_dir(&d).unwrap();
        for p in ["a", "a/b", "a/b/c"] {
            assert_eq!(mode(&t.path().join(p)), 0o700, "{p}");
        }
    }

    #[test]
    fn existing_loose_dir_is_repaired_and_owner_bits_kept() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("data");
        std::fs::create_dir(&d).unwrap();
        set_mode(&d, 0o755);
        ensure_private_dir(&d).unwrap();
        assert_eq!(mode(&d), 0o700);

        let f = d.join("reel.db");
        std::fs::write(&f, b"x").unwrap();
        set_mode(&f, 0o644);
        assert!(repair(&f));
        assert_eq!(mode(&f), 0o600);
        // already private: nothing to do; missing: fine
        assert!(!repair(&f));
        assert!(!repair(&d.join("missing")));
    }

    #[test]
    fn create_private_file_is_0600_and_keeps_existing() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("reel.db");
        create_private_file(&f).unwrap();
        assert_eq!(mode(&f), 0o600);
        std::fs::write(&f, b"keep").unwrap();
        create_private_file(&f).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"keep");
    }

    #[test]
    fn write_private_replaces_a_loose_file_and_leaves_no_temp() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("signing.key");
        std::fs::write(&f, b"old").unwrap();
        set_mode(&f, 0o644);
        write_private(&f, b"new").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"new");
        assert_eq!(mode(&f), 0o600);
        let names: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("signing.key")]);
    }

    /// Watch the directory while the file is written over and over: no observer may ever see the key (or its
    /// temp file) with group/other bits set.
    #[test]
    fn write_private_has_no_readable_window() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().to_path_buf();
        let f = dir.join("signing.key");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let (dir, stop) = (dir.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut seen = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for e in std::fs::read_dir(&dir).unwrap().flatten() {
                        if let Ok(m) = e.metadata() {
                            seen += 1;
                            assert_eq!(
                                m.permissions().mode() & 0o077,
                                0,
                                "{:?} was readable by others",
                                e.file_name()
                            );
                        }
                    }
                }
                seen
            })
        };
        for i in 0..300u32 {
            write_private(&f, format!("{i:064}").as_bytes()).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(watcher.join().unwrap() > 0);
        assert_eq!(mode(&f), 0o600);
    }
}
