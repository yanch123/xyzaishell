use super::super::{Result, StorageError as E};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub(crate) struct PrivateDirectory {
    directory: File,
}

fn name(value: &str) -> Result<CString> {
    if value.is_empty()
        || value.len() > 180
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(E::InvalidPath);
    }
    CString::new(value).map_err(|_| E::InvalidPath)
}
fn c_path(value: &std::ffi::OsStr) -> Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| E::InvalidPath)
}
fn owned(fd: i32) -> Result<File> {
    if fd < 0 {
        return Err(E::Unavailable);
    }
    // SAFETY: successful open/openat returns a newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn local(file: &File) -> Result<()> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: live fd and writable output, read only on success.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(E::UnsafeFile);
    }
    let stat = unsafe { stat.assume_init() };
    if !matches!(
        stat.f_type as u64,
        0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630
    ) {
        return Err(E::UnsupportedFilesystem);
    }
    Ok(())
}
fn directory(file: &File, private: bool) -> Result<()> {
    let meta = file.metadata().map_err(|_| E::UnsafeFile)?;
    if !meta.is_dir() {
        return Err(E::UnsafeFile);
    }
    if private {
        if meta.uid() != uid() || meta.mode() & 0o077 != 0 {
            return Err(E::UnsafeFile);
        }
    } else if (meta.uid() != uid() && meta.uid() != 0)
        || (meta.mode() & 0o022 != 0 && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0))
    {
        return Err(E::UnsafeFile);
    }
    local(file)
}
fn regular(file: &File, max: usize) -> Result<std::fs::Metadata> {
    let meta = file.metadata().map_err(|_| E::UnsafeFile)?;
    if !meta.is_file() || meta.uid() != uid() || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
        return Err(E::UnsafeFile);
    }
    if meta.len() > max as u64 {
        return Err(E::TooLarge);
    }
    local(file)?;
    Ok(meta)
}
fn same(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
fn open_directory(parent: &File, child: &CString, create: bool) -> Result<File> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let mut fd = unsafe { libc::openat(parent.as_raw_fd(), child.as_ptr(), flags) };
    if fd < 0 && create && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        let made = unsafe { libc::mkdirat(parent.as_raw_fd(), child.as_ptr(), 0o700) };
        if made != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(E::Unavailable);
        }
        if made == 0 {
            parent.sync_all().map_err(|_| E::SaveUncertain)?;
        }
        fd = unsafe { libc::openat(parent.as_raw_fd(), child.as_ptr(), flags) };
    }
    owned(fd)
}
impl PrivateDirectory {
    pub fn open_or_create(path: &Path) -> Result<Self> {
        if !path.is_absolute()
            || path
                .as_os_str()
                .as_bytes()
                .split(|c| *c == b'/')
                .any(|s| s == b"." || s == b"..")
        {
            return Err(E::InvalidPath);
        }
        let parts: Vec<_> = path.components().collect();
        if parts.len() < 2
            || parts[1..]
                .iter()
                .any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err(E::InvalidPath);
        }
        let mut current = File::open("/").map_err(|_| E::Unavailable)?;
        directory(&current, false)?;
        for component in &parts[1..] {
            current = open_directory(&current, &c_path(component.as_os_str())?, true)?;
            directory(&current, false)?;
        }
        directory(&current, true)?;
        Ok(Self { directory: current })
    }
    pub fn child(&self, child: &str) -> Result<Self> {
        directory(&self.directory, true)?;
        let file = open_directory(&self.directory, &name(child)?, true)?;
        directory(&file, true)?;
        Ok(Self { directory: file })
    }
    /// Securely creates only a new empty SQLite database; never truncates or
    /// replaces an existing file. The caller initializes schema transactionally.
    pub fn prepare_database_file(&self, filename: &str, max: usize) -> Result<bool> {
        if !matches!(filename, "history.sqlite3" | "shell-log.sqlite3") {
            return Err(E::InvalidPath);
        }
        directory(&self.directory, true)?;
        let filename_c = name(filename)?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                filename_c.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
            if !self.validate_database_file(filename, max)? {
                return Err(E::Conflict);
            }
            return Ok(false);
        }
        let file = owned(fd)?;
        regular(&file, max)?;
        file.sync_all().map_err(|_| E::SaveUncertain)?;
        self.directory.sync_all().map_err(|_| E::SaveUncertain)?;
        directory(&self.directory, true)?;
        Ok(true)
    }
    /// Metadata-only inspection compatible with a live SQLite connection.
    /// It grants no permission to store raw secrets or to use another filename.
    pub fn validate_database_file(&self, filename: &str, max: usize) -> Result<bool> {
        if !matches!(
            filename,
            "history.sqlite3"
                | "history.sqlite3-journal"
                | "history.sqlite3-wal"
                | "history.sqlite3-shm"
                | "shell-log.sqlite3"
                | "shell-log.sqlite3-journal"
                | "shell-log.sqlite3-wal"
                | "shell-log.sqlite3-shm"
        ) {
            return Err(E::InvalidPath);
        }
        directory(&self.directory, true)?;
        let filename = name(filename)?;
        // O_PATH never reads data or invokes a device's open handler. NOFOLLOW
        // opens a symlink itself, which regular() then rejects via fstat.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                filename.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(false);
        }
        let file = owned(fd)?;
        regular(&file, max)?;
        directory(&self.directory, true)?;
        Ok(true)
    }
    pub fn read(&self, filename: &str, max: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
        directory(&self.directory, true)?;
        let filename = name(filename)?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                filename.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        let file = owned(fd)?;
        let before = regular(&file, max)?;
        let mut contents = Zeroizing::new(Vec::new());
        (&file)
            .take(max as u64 + 1)
            .read_to_end(&mut contents)
            .map_err(|_| E::Unavailable)?;
        if contents.len() > max {
            return Err(E::TooLarge);
        }
        let after = regular(&file, max)?;
        directory(&self.directory, true)?;
        if !same(&before, &after) {
            return Err(E::Conflict);
        }
        Ok(Some(contents))
    }
    pub fn update<T>(
        &self,
        filename: &str,
        max: usize,
        apply: impl FnOnce(Option<&[u8]>) -> Result<(T, Option<Zeroizing<Vec<u8>>>)>,
    ) -> Result<T> {
        directory(&self.directory, true)?;
        let lockname = name(&format!(".{filename}.lock"))?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                lockname.as_ptr(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
                0o600,
            )
        };
        let lock = owned(fd)?;
        regular(&lock, 0)?;
        let start = Instant::now();
        loop {
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(E::Unavailable);
            }
            if start.elapsed() >= Duration::from_secs(5) {
                return Err(E::Busy);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // The stable lock is intentionally never removed. A replaced lock file
        // is rejected when its inode no longer names the locked descriptor.
        let lock_meta = regular(&lock, 0)?;
        let mut at = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                lockname.as_ptr(),
                at.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(E::UnsafeFile);
        }
        let at = unsafe { at.assume_init() };
        if at.st_ino != lock_meta.ino() || at.st_dev != lock_meta.dev() {
            return Err(E::Conflict);
        }
        let before = self.read(filename, max)?;
        let (result, replacement) = apply(before.as_deref().map(Vec::as_slice))?;
        let Some(bytes) = replacement else {
            return Ok(result);
        };
        if bytes.len() > max {
            return Err(E::TooLarge);
        }
        // Detect cooperative writers and most editor races. Editors that ignore
        // the lock can still race the final rename; this is not editor isolation.
        let current = self.read(filename, max)?;
        if current.as_deref() != before.as_deref() {
            return Err(E::Conflict);
        }
        self.replace(filename, &bytes)?;
        match self.read(filename, max) {
            Ok(Some(saved)) if saved.as_slice() == bytes.as_slice() => Ok(result),
            _ => Err(E::SaveUncertain),
        }
    }
    fn replace(&self, filename: &str, bytes: &[u8]) -> Result<()> {
        let target = name(filename)?;
        let temporary = name(&format!(".{}.tmp", uuid::Uuid::new_v4()))?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        let mut file = owned(fd)?;
        let prepare = (|| {
            regular(&file, bytes.len())?;
            file.write_all(bytes).map_err(|_| E::Unavailable)?;
            file.sync_all().map_err(|_| E::Unavailable)?;
            directory(&self.directory, true)?;
            regular(&file, bytes.len())?;
            Ok(())
        })();
        if let Err(e) = prepare {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), temporary.as_ptr(), 0);
            }
            return Err(e);
        }
        if unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                temporary.as_ptr(),
                self.directory.as_raw_fd(),
                target.as_ptr(),
            )
        } != 0
        {
            // EIO and filesystem failures need not establish an uncommitted
            // state. Preserve the private recovery file and require reread.
            return Err(E::SaveUncertain);
        }
        self.directory.sync_all().map_err(|_| E::SaveUncertain)
    }
}
