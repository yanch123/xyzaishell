//! Windows storage uses pinned directory handles and a permanent byte-range
//! lock, not a lock on the data file that is replaced. Every new object has a
//! protected owner/SYSTEM/Administrators DACL at creation. Existing ACLs are
//! inspected, never repaired. Only local NTFS/ReFS volumes are accepted.
//!
//! The temporary file is flushed before handle-relative rename, then flushed
//! again and reread. Success establishes the observable committed value, not a
//! promise about sudden power loss or a device that ignores flushes. Any error
//! after attempting the commit is SaveUncertain; callers must reload and must
//! not automatically delete credentials that the new document might reference.
//!
//! As with the private-file reader, a malicious same-user process, an
//! administrator, or a filesystem driver is outside this security boundary.
//! Noncooperative editors can race the final rename despite the last reread.

use super::super::{Result, StorageError as E};
use crate::windows_private_file as checked;
use std::{
    ffi::c_void,
    fs::File,
    io::{Read, Write},
    mem::{offset_of, size_of},
    os::windows::io::{AsRawHandle, OwnedHandle},
    path::Path,
    ptr::{null, null_mut},
    sync::Arc,
    time::{Duration, Instant},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileDispositionInformation, FileRenameInformation, NtCreateFile, NtSetInformationFile,
            FILE_CREATE, FILE_DIRECTORY_FILE, FILE_DISPOSITION_INFORMATION,
            FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
            FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT, FILE_WRITE_THROUGH,
        },
    },
    Win32::{
        Foundation::{
            GetLastError, LocalFree, ERROR_LOCK_VIOLATION, ERROR_SUCCESS, HANDLE,
            OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, STATUS_OBJECT_NAME_NOT_FOUND, UNICODE_STRING,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            GetSecurityDescriptorControl, DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        },
        Storage::FileSystem::{
            GetVolumeInformationByHandleW, LockFileEx, UnlockFileEx, BY_HANDLE_FILE_INFORMATION,
            DELETE, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_TRAVERSE, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, READ_CONTROL,
            SYNCHRONIZE,
        },
        System::IO::{IO_STATUS_BLOCK, OVERLAPPED},
    },
};
use zeroize::Zeroizing;

pub(crate) struct PrivateDirectory {
    // Keep every ancestor pinned without FILE_SHARE_DELETE. Cloned children
    // retain those pins even after their parent PrivateDirectory is dropped.
    directories: Vec<Arc<OwnedHandle>>,
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: only successful LocalAlloc-returning APIs construct this.
        unsafe { LocalFree(self.0) };
    }
}

fn mapped(error: &'static str) -> E {
    match error {
        "PRIVATE_PATH_INVALID" => E::InvalidPath,
        "PRIVATE_FILE_TOO_LARGE" => E::TooLarge,
        "PRIVATE_FILE_OPEN_FAILED" | "PRIVATE_FILE_READ_FAILED" => E::Unavailable,
        _ => E::UnsafeFile,
    }
}

fn name(value: &str) -> Result<Vec<u16>> {
    if value.is_empty()
        || value.len() > 180
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(E::InvalidPath);
    }
    // Reuse the DOS alias/alternate-stream/trailing-dot checks as well.
    let (_, mut parts) =
        checked::checked_components(Path::new(&format!(r"C:\{value}"))).map_err(mapped)?;
    if parts.len() != 1 {
        return Err(E::InvalidPath);
    }
    Ok(parts.remove(0))
}

fn descriptor(identity: &checked::Identity, directory: bool) -> Result<LocalAllocation> {
    let mut sid_text = null_mut();
    // SAFETY: a validated effective SID and writable output pointer; successful
    // conversion transfers a NUL-terminated, LocalFree-owned allocation.
    if unsafe { ConvertSidToStringSidW(identity.sid(), &mut sid_text) } == 0 {
        return Err(E::Unavailable);
    }
    let _sid = LocalAllocation(sid_text.cast());
    let mut length = 0;
    // SID strings are bounded by Windows. Bound our scan defensively too.
    while length < 256 && unsafe { *sid_text.add(length) } != 0 {
        length += 1;
    }
    if length == 256 {
        return Err(E::UnsafeFile);
    }
    // SAFETY: successful conversion supplies this initialized string range.
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
        .map_err(|_| E::UnsafeFile)?;
    let inherit = if directory { "OICI" } else { "" };
    let sddl: Vec<u16> =
        format!("O:{sid}D:P(A;{inherit};FA;;;{sid})(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)")
            .encode_utf16()
            .chain(Some(0))
            .collect();
    let mut sd = null_mut();
    // SAFETY: NUL-terminated SDDL, revision 1, live writable output.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut sd, null_mut())
    } == 0
        || sd.is_null()
    {
        return Err(E::Unavailable);
    }
    Ok(LocalAllocation(sd))
}

fn private(handle: HANDLE, identity: &checked::Identity) -> Result<()> {
    checked::validate_private_acl(handle, identity.sid()).map_err(mapped)?;
    let mut sd = null_mut();
    // SAFETY: live READ_CONTROL handle; descriptor allocation remains owned
    // until its control flags have been checked.
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    } != ERROR_SUCCESS
        || sd.is_null()
    {
        return Err(E::UnsafeFile);
    }
    let _allocation = LocalAllocation(sd);
    let (mut control, mut revision) = (0, 0);
    // SAFETY: GetSecurityInfo returned a complete live security descriptor.
    if unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(E::UnsafeFile);
    }
    Ok(())
}

fn local_directory(handle: &OwnedHandle) -> Result<()> {
    checked::validate_directory(handle).map_err(mapped)?;
    let mut filesystem = [0u16; 32];
    // SAFETY: valid local directory and complete writable filesystem-name buffer.
    if unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    } == 0
    {
        return Err(E::UnsupportedFilesystem);
    }
    let length = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .ok_or(E::UnsafeFile)?;
    if !["NTFS", "ReFS"].iter().any(|value| {
        value
            .encode_utf16()
            .eq(filesystem[..length].iter().copied())
    }) {
        return Err(E::UnsupportedFilesystem);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum OpenKind {
    Directory,
    Read,
    Lock,
    Temporary,
    DatabaseMetadata,
    DatabaseCreate,
}

fn open_relative(
    parent: &OwnedHandle,
    name: &[u16],
    kind: OpenKind,
    sd: Option<&LocalAllocation>,
) -> Result<Option<OwnedHandle>> {
    open_relative_with_created(parent, name, kind, sd).map(|(handle, _)| handle)
}

fn open_relative_with_created(
    parent: &OwnedHandle,
    name: &[u16],
    kind: OpenKind,
    sd: Option<&LocalAllocation>,
) -> Result<(Option<OwnedHandle>, bool)> {
    let mut wide = name.to_vec();
    let length = u16::try_from(wide.len() * size_of::<u16>()).map_err(|_| E::InvalidPath)?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: sd.map_or(null(), |value| value.0.cast()),
        ..Default::default()
    };
    let (access, sharing, disposition, options) = match kind {
        OpenKind::Directory => (
            FILE_READ_ATTRIBUTES | FILE_TRAVERSE | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_IF,
            FILE_DIRECTORY_FILE,
        ),
        OpenKind::Read => (
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        ),
        OpenKind::Lock => (
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_IF,
            FILE_NON_DIRECTORY_FILE,
        ),
        OpenKind::DatabaseMetadata => (
            FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        ),
        OpenKind::DatabaseCreate => (
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN_IF,
            FILE_NON_DIRECTORY_FILE | FILE_WRITE_THROUGH,
        ),
        OpenKind::Temporary => (
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE,
            0,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE | FILE_WRITE_THROUGH,
        ),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = null_mut();
    // SAFETY: one checked component relative to a pinned directory; all input
    // buffers and creation descriptor outlive this synchronous call. OPEN_IF
    // never replaces/repairs an existing object's descriptor or contents.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            sharing,
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            null(),
            0,
        )
    };
    if result == STATUS_OBJECT_NAME_NOT_FOUND
        && matches!(kind, OpenKind::Read | OpenKind::DatabaseMetadata)
    {
        return Ok((None, false));
    }
    if result < 0 {
        return Err(E::Unavailable);
    }
    // NtCreateFile documents IO_STATUS_BLOCK.Information == FILE_CREATED (2)
    // only when this operation created the file; OPEN_IF never truncates.
    let created = status.Information == 2;
    checked::owned_handle(handle)
        .map(|handle| (Some(handle), created))
        .map_err(mapped)
}

fn regular(file: &File, identity: &checked::Identity, max: usize) -> Result<()> {
    checked::validate_file(file, max).map_err(mapped)?;
    private(file.as_raw_handle(), identity)
}

fn unchanged(a: &BY_HANDLE_FILE_INFORMATION, b: &BY_HANDLE_FILE_INFORMATION) -> bool {
    a.dwVolumeSerialNumber == b.dwVolumeSerialNumber
        && a.nFileIndexHigh == b.nFileIndexHigh
        && a.nFileIndexLow == b.nFileIndexLow
        && a.nFileSizeHigh == b.nFileSizeHigh
        && a.nFileSizeLow == b.nFileSizeLow
        && a.ftLastWriteTime.dwHighDateTime == b.ftLastWriteTime.dwHighDateTime
        && a.ftLastWriteTime.dwLowDateTime == b.ftLastWriteTime.dwLowDateTime
}

impl PrivateDirectory {
    fn handle(&self) -> &OwnedHandle {
        self.directories
            .last()
            .expect("private directory has a pinned handle")
    }

    fn validate(&self, identity: &checked::Identity) -> Result<()> {
        local_directory(self.handle())?;
        private(self.handle().as_raw_handle(), identity)
    }

    pub fn open_or_create(path: &Path) -> Result<Self> {
        let (root_name, parts) = checked::checked_components(path).map_err(mapped)?;
        let identity = checked::Identity::effective().map_err(mapped)?;
        let sd = descriptor(&identity, true)?;
        let root = checked::open_drive_root(&root_name).map_err(mapped)?;
        local_directory(&root)?;
        let mut directories = vec![Arc::new(root)];
        for part in parts {
            let handle = open_relative(
                directories.last().unwrap(),
                &part,
                OpenKind::Directory,
                Some(&sd),
            )?
            .ok_or(E::Unavailable)?;
            local_directory(&handle)?;
            directories.push(Arc::new(handle));
        }
        let directory = Self { directories };
        // Ancestors may be shared system/profile directories. The actual app
        // directory must be owned by this user and protected/private; do not
        // silently chmod/repair a pre-existing permissive directory.
        directory.validate(&identity)?;
        Ok(directory)
    }

    pub fn child(&self, child: &str) -> Result<Self> {
        let identity = checked::Identity::effective().map_err(mapped)?;
        self.validate(&identity)?;
        let sd = descriptor(&identity, true)?;
        let handle = open_relative(self.handle(), &name(child)?, OpenKind::Directory, Some(&sd))?
            .ok_or(E::Unavailable)?;
        let mut directories = self.directories.clone();
        directories.push(Arc::new(handle));
        let child = Self { directories };
        child.validate(&identity)?;
        self.validate(&identity)?;
        Ok(child)
    }

    /// Precreates a private database without replacement. Existing zero-length
    /// files return false so the history layer can flag possible data loss.
    pub fn prepare_database_file(&self, filename: &str, max: usize) -> Result<bool> {
        if filename != "history.sqlite3" {
            return Err(E::InvalidPath);
        }
        let identity = checked::Identity::effective().map_err(mapped)?;
        self.validate(&identity)?;
        let sd = descriptor(&identity, false)?;
        let (handle, created) = open_relative_with_created(
            self.handle(),
            &name(filename)?,
            OpenKind::DatabaseCreate,
            Some(&sd),
        )?;
        let file = File::from(handle.ok_or(E::Unavailable)?);
        checked::validate_file(&file, max).map_err(mapped)?;
        checked::validate_private_acl(file.as_raw_handle(), identity.sid()).map_err(mapped)?;
        if created {
            // Newly created main databases must have our protected descriptor.
            private(file.as_raw_handle(), &identity)?;
            file.sync_all().map_err(|_| E::SaveUncertain)?;
        }
        self.validate(&identity)?;
        Ok(created)
    }
    /// Checks metadata using SQLite-compatible sharing. SQLite's sidecar files
    /// may inherit a private ACL from our protected directory; their effective
    /// grants still must be limited to this user, SYSTEM and administrators.
    pub fn validate_database_file(&self, filename: &str, max: usize) -> Result<bool> {
        if !matches!(
            filename,
            "history.sqlite3"
                | "history.sqlite3-journal"
                | "history.sqlite3-wal"
                | "history.sqlite3-shm"
        ) {
            return Err(E::InvalidPath);
        }
        let identity = checked::Identity::effective().map_err(mapped)?;
        self.validate(&identity)?;
        let Some(handle) = open_relative(
            self.handle(),
            &name(filename)?,
            OpenKind::DatabaseMetadata,
            None,
        )?
        else {
            return Ok(false);
        };
        let file = File::from(handle);
        checked::validate_file(&file, max).map_err(mapped)?;
        checked::validate_private_acl(file.as_raw_handle(), identity.sid()).map_err(mapped)?;
        self.validate(&identity)?;
        Ok(true)
    }
    pub fn read(&self, filename: &str, max: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let identity = checked::Identity::effective().map_err(mapped)?;
        self.validate(&identity)?;
        let Some(handle) = open_relative(self.handle(), &name(filename)?, OpenKind::Read, None)?
        else {
            return Ok(None);
        };
        let mut file = File::from(handle);
        regular(&file, &identity, max)?;
        let before = checked::file_information(file.as_raw_handle()).map_err(mapped)?;
        let limit = u64::try_from(max)
            .ok()
            .and_then(|v| v.checked_add(1))
            .ok_or(E::TooLarge)?;
        let mut contents = Zeroizing::new(Vec::new());
        (&mut file)
            .take(limit)
            .read_to_end(&mut contents)
            .map_err(|_| E::Unavailable)?;
        if contents.len() > max {
            return Err(E::TooLarge);
        }
        regular(&file, &identity, max)?;
        self.validate(&identity)?;
        let after = checked::file_information(file.as_raw_handle()).map_err(mapped)?;
        if !unchanged(&before, &after) {
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
        let target = name(filename)?;
        let identity = checked::Identity::effective().map_err(mapped)?;
        self.validate(&identity)?;
        let sd = descriptor(&identity, false)?;
        let lock_name = name(&format!(".{filename}.lock"))?;
        let lock = File::from(
            open_relative(self.handle(), &lock_name, OpenKind::Lock, Some(&sd))?
                .ok_or(E::Unavailable)?,
        );
        regular(&lock, &identity, 0)?;
        let _lock = FileLock::acquire(lock)?;
        self.validate(&identity)?;
        regular(&_lock.file, &identity, 0)?;
        let before = self.read(filename, max)?;
        let (result, replacement) = apply(before.as_deref().map(Vec::as_slice))?;
        let Some(bytes) = replacement else {
            return Ok(result);
        };
        if bytes.len() > max {
            return Err(E::TooLarge);
        }
        let temporary_name = name(&format!(".{}.tmp", uuid::Uuid::new_v4()))?;
        let file = File::from(
            open_relative(
                self.handle(),
                &temporary_name,
                OpenKind::Temporary,
                Some(&sd),
            )?
            .ok_or(E::Unavailable)?,
        );
        let mut temporary = Temporary {
            file,
            committed: false,
        };
        regular(&temporary.file, &identity, max)?;
        temporary
            .file
            .write_all(&bytes)
            .map_err(|_| E::Unavailable)?;
        temporary.file.sync_all().map_err(|_| E::Unavailable)?;
        regular(&temporary.file, &identity, max)?;
        self.validate(&identity)?;
        // Detect editor changes as late as possible before commit. The stable
        // lock serializes app processes; editors must cooperate for isolation.
        let current = self.read(filename, max)?;
        if current.as_deref() != before.as_deref() {
            return Err(E::Conflict);
        }
        regular(&_lock.file, &identity, 0)?;
        temporary.commit(self.handle(), &target)?;
        // Drop the exclusive temporary-file handle before reopening the final
        // name. Never roll back or delete the target after an uncertain commit.
        drop(temporary);
        match self.read(filename, max) {
            Ok(Some(saved)) if saved.as_slice() == bytes.as_slice() => Ok(result),
            _ => Err(E::SaveUncertain),
        }
    }
}

struct FileLock {
    file: File,
}
impl FileLock {
    fn acquire(file: File) -> Result<Self> {
        let start = Instant::now();
        loop {
            let mut overlapped = OVERLAPPED::default();
            // SAFETY: live synchronous file and initialized OVERLAPPED. Locking
            // the first byte beyond EOF is supported and does not write data.
            if unsafe {
                LockFileEx(
                    file.as_raw_handle(),
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    1,
                    0,
                    &mut overlapped,
                )
            } != 0
            {
                return Ok(Self { file });
            }
            if unsafe { GetLastError() } != ERROR_LOCK_VIOLATION {
                return Err(E::Unavailable);
            }
            if start.elapsed() >= Duration::from_secs(5) {
                return Err(E::Busy);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: release exactly the range acquired on this same live handle.
        unsafe { UnlockFileEx(self.file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    }
}

struct Temporary {
    file: File,
    committed: bool,
}
impl Temporary {
    fn commit(&mut self, parent: &OwnedHandle, target: &[u16]) -> Result<()> {
        // Aligned, zeroed allocation for the complete variable-length struct.
        let length = size_of::<FILE_RENAME_INFORMATION>() + std::mem::size_of_val(target);
        let mut buffer = vec![0usize; length.div_ceil(size_of::<usize>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: allocation has suitable alignment and at least struct + name
        // bytes. The target is one checked relative name, never a full path.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = true;
            (*info).RootDirectory = parent.as_raw_handle();
            (*info).FileNameLength = std::mem::size_of_val(target) as u32;
            std::ptr::copy_nonoverlapping(
                target.as_ptr(),
                buffer
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(offset_of!(FILE_RENAME_INFORMATION, FileName))
                    .cast::<u16>(),
                target.len(),
            );
        }
        let mut status = IO_STATUS_BLOCK::default();
        // Once rename is attempted, do not risk deleting a file whose final
        // location is uncertain. A failure may leave a private orphan temp.
        self.committed = true;
        // SAFETY: the live DELETE-capable source, pinned destination directory,
        // and complete aligned rename buffer remain valid throughout the call.
        if unsafe {
            NtSetInformationFile(
                self.file.as_raw_handle(),
                &mut status,
                info.cast(),
                length as u32,
                FileRenameInformation,
            )
        } < 0
        {
            return Err(E::SaveUncertain);
        }
        self.file.sync_all().map_err(|_| E::SaveUncertain)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let disposition = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: mark only our already-open CREATE_NEW temporary for deletion;
        // never reopen a pathname during cleanup. Failure leaves a private
        // orphan, not a broad-permission or partially-written target file.
        unsafe {
            NtSetInformationFile(
                self.file.as_raw_handle(),
                &mut status,
                (&disposition as *const FILE_DISPOSITION_INFORMATION).cast(),
                size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
                FileDispositionInformation,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::PathBuf, process::Command};
    use windows_sys::Win32::{
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateDirectoryW, CreateFileW, CREATE_NEW, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING,
        },
        System::{
            Ioctl::FSCTL_SET_REPARSE_POINT, SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
            IO::DeviceIoControl,
        },
    };

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("xyz-store-test-{}", uuid::Uuid::new_v4()));
            drop(PrivateDirectory::open_or_create(&path).unwrap());
            Self(path)
        }
        fn directory(&self) -> PrivateDirectory {
            PrivateDirectory::open_or_create(&self.0).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn synthetic_descriptor(broad: bool, protected: bool) -> LocalAllocation {
        let identity = checked::Identity::effective().unwrap();
        let mut sid_text = null_mut();
        // SAFETY: test identity and writable output, freed after copying.
        assert_ne!(
            unsafe { ConvertSidToStringSidW(identity.sid(), &mut sid_text) },
            0
        );
        let _allocation = LocalAllocation(sid_text.cast());
        let mut length = 0;
        // SAFETY: a successful SID conversion returns a terminated UTF-16 string.
        let sid = unsafe {
            while *sid_text.add(length) != 0 {
                length += 1;
            }
            String::from_utf16(std::slice::from_raw_parts(sid_text, length)).unwrap()
        };
        let extra = if broad { "(A;;FR;;;WD)" } else { "" };
        let protect = if protected { "P" } else { "" };
        let sddl = wide(OsStr::new(&format!(
            "O:{sid}D:{protect}(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){extra}"
        )));
        let mut sd = null_mut();
        // SAFETY: only create a descriptor for a new disposable test object.
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut sd,
                    null_mut(),
                )
            },
            0
        );
        LocalAllocation(sd)
    }

    fn create_fixture_file(path: &Path, broad: bool, contents: &[u8]) {
        let sd = synthetic_descriptor(broad, true);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let path = wide(path.as_os_str());
        // SAFETY: CREATE_NEW writes only a uniquely named disposable fixture.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        let mut file = File::from(checked::owned_handle(handle).unwrap());
        file.write_all(contents).unwrap();
    }

    #[test]
    fn creates_protected_files_and_replaces_without_partial_contents() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        assert_eq!(directory.read("profiles.toml", 32).unwrap(), None);
        assert_eq!(
            directory.update("profiles.toml", 32, |before| {
                assert!(before.is_none());
                Ok((7, Some(Zeroizing::new(b"first".to_vec()))))
            }),
            Ok(7)
        );
        directory
            .update("profiles.toml", 32, |before| {
                assert_eq!(before, Some(b"first".as_slice()));
                Ok(((), Some(Zeroizing::new(b"replacement".to_vec()))))
            })
            .unwrap();
        assert_eq!(
            directory
                .read("profiles.toml", 32)
                .unwrap()
                .unwrap()
                .as_slice(),
            b"replacement"
        );
        assert_eq!(directory.read("profiles.toml", 3), Err(E::TooLarge));
        assert!(fixture.0.join(".profiles.toml.lock").exists());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
        let child = directory.child("secrets").unwrap();
        drop(directory);
        child
            .update("entry", 32, |_| {
                Ok(((), Some(Zeroizing::new(b"synthetic".to_vec()))))
            })
            .unwrap();
        assert_eq!(
            child.read("entry", 32).unwrap().unwrap().as_slice(),
            b"synthetic"
        );
    }

    #[test]
    fn rejects_broad_existing_directory_without_repair() {
        let fixture = Fixture::new();
        let path = fixture.0.join("broad");
        let sd = synthetic_descriptor(true, true);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let path_wide = wide(path.as_os_str());
        // SAFETY: create a fresh disposable directory, never change an ACL.
        assert_ne!(
            unsafe { CreateDirectoryW(path_wide.as_ptr(), &attributes) },
            0
        );
        assert!(matches!(
            PrivateDirectory::open_or_create(&path),
            Err(E::UnsafeFile)
        ));
        assert!(matches!(
            fixture.directory().child("broad"),
            Err(E::UnsafeFile)
        ));
    }

    #[test]
    fn rejects_unprotected_existing_directory_without_repair() {
        let fixture = Fixture::new();
        let path = fixture.0.join("inherited");
        let sd = synthetic_descriptor(false, false);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let path_wide = wide(path.as_os_str());
        // SAFETY: intentionally inheritable ACL on a new disposable fixture.
        assert_ne!(
            unsafe { CreateDirectoryW(path_wide.as_ptr(), &attributes) },
            0
        );
        assert!(matches!(
            PrivateDirectory::open_or_create(&path),
            Err(E::UnsafeFile)
        ));
    }

    #[test]
    fn rejects_broad_data_and_lock_acl_and_hardlinks() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        create_fixture_file(&fixture.0.join("broad"), true, b"synthetic");
        assert_eq!(directory.read("broad", 32), Err(E::UnsafeFile));
        assert_eq!(
            directory.update("broad", 32, |_| Ok(((), Some(Zeroizing::new(vec![]))))),
            Err(E::UnsafeFile)
        );
        create_fixture_file(&fixture.0.join(".blocked.lock"), true, b"");
        assert_eq!(
            directory.update("blocked", 32, |_| Ok(((), None))),
            Err(E::UnsafeFile)
        );
        create_fixture_file(&fixture.0.join("linked"), false, b"synthetic");
        std::fs::hard_link(fixture.0.join("linked"), fixture.0.join("alias")).unwrap();
        assert_eq!(directory.read("linked", 32), Err(E::UnsafeFile));
    }

    #[test]
    fn checks_bounds_and_conflicts_before_committing_and_cleans_temp() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        create_fixture_file(&fixture.0.join("profiles"), false, b"old");
        assert_eq!(
            directory.update("profiles", 4, |_| Ok((
                (),
                Some(Zeroizing::new(b"large".to_vec()))
            ))),
            Err(E::TooLarge)
        );
        assert_eq!(std::fs::read(fixture.0.join("profiles")).unwrap(), b"old");
        let outcome = directory.update("profiles", 32, |_| {
            std::fs::write(fixture.0.join("profiles"), b"editor").unwrap();
            Ok(((), Some(Zeroizing::new(b"app".to_vec()))))
        });
        assert_eq!(outcome, Err(E::Conflict));
        assert_eq!(
            std::fs::read(fixture.0.join("profiles")).unwrap(),
            b"editor"
        );
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
        let outcome: Result<()> = directory.update("profiles", 32, |_| Err(E::Conflict));
        assert_eq!(outcome, Err(E::Conflict));
    }

    #[test]
    fn uncertain_rename_does_not_remove_or_truncate_the_target() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let directory = fixture.directory();
        create_fixture_file(&fixture.0.join("profiles"), false, b"old");
        // An existing reader that denies deletion deliberately prevents the
        // commit. The final target must survive, even on the uncertain path.
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(fixture.0.join("profiles"))
            .unwrap();
        assert_eq!(
            directory.update("profiles", 32, |_| {
                Ok(((), Some(Zeroizing::new(b"replacement".to_vec()))))
            }),
            Err(E::SaveUncertain)
        );
        drop(reader);
        assert_eq!(
            directory.read("profiles", 32).unwrap().unwrap().as_slice(),
            b"old"
        );
        // Conservative failure handling can leave a fully private orphan;
        // verify it through the same owner/protected-DACL read boundary.
        for entry in std::fs::read_dir(&fixture.0).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if name.ends_with(".tmp") {
                assert_eq!(
                    directory.read(&name, 32).unwrap().unwrap().as_slice(),
                    b"replacement"
                );
            }
        }
    }

    #[test]
    fn pins_directory_and_stable_lock_against_replacement() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        assert!(std::fs::rename(&fixture.0, fixture.0.with_extension("renamed")).is_err());
        directory
            .update("profiles", 32, |_| {
                assert!(std::fs::remove_file(fixture.0.join(".profiles.lock")).is_err());
                assert!(std::fs::rename(
                    fixture.0.join(".profiles.lock"),
                    fixture.0.join("other-lock")
                )
                .is_err());
                Ok(((), None))
            })
            .unwrap();
    }

    #[test]
    fn rejects_alias_and_remote_paths_and_filenames() {
        for path in [
            r"C:relative",
            r"\\server\share\store",
            r"\\?\C:\store",
            r"C:\x\..\store",
            r"C:\x\NUL",
            r"C:\x\store:stream",
        ] {
            assert!(matches!(
                PrivateDirectory::open_or_create(Path::new(path)),
                Err(E::InvalidPath)
            ));
        }
        let fixture = Fixture::new();
        let directory = fixture.directory();
        for filename in [
            "",
            ".",
            "..",
            "NUL",
            "COM1.txt",
            "trailing.",
            "name:stream",
            "a/b",
            "a\\b",
        ] {
            assert_eq!(directory.read(filename, 32), Err(E::InvalidPath));
        }
    }

    #[test]
    fn refuses_intermediate_directory_junction() {
        let fixture = Fixture::new();
        let junction = fixture.0.join("junction");
        std::fs::create_dir(&junction).unwrap();
        let path = wide(junction.as_os_str());
        // SAFETY: manipulate only this new, empty, disposable junction fixture.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let handle = checked::owned_handle(raw).unwrap();
        let substitute = wide(OsStr::new(&format!(r"\??\{}", fixture.0.display())));
        let print = wide(fixture.0.as_os_str());
        let payload = 8 + (substitute.len() + print.len()) * 2;
        let mut bytes = Vec::new();
        bytes.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        for value in [
            payload as u16,
            0,
            0,
            ((substitute.len() - 1) * 2) as u16,
            (substitute.len() * 2) as u16,
            ((print.len() - 1) * 2) as u16,
        ] {
            bytes.extend(value.to_le_bytes());
        }
        for unit in substitute.into_iter().chain(print) {
            bytes.extend(unit.to_le_bytes());
        }
        let mut returned = 0;
        // SAFETY: complete bounded reparse buffer and owned test-directory handle.
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    handle.as_raw_handle(),
                    FSCTL_SET_REPARSE_POINT,
                    bytes.as_ptr().cast(),
                    bytes.len() as u32,
                    null_mut(),
                    0,
                    &mut returned,
                    null_mut(),
                )
            },
            0
        );
        drop(handle);
        assert!(PrivateDirectory::open_or_create(&junction.join("store")).is_err());
        assert!(fixture.directory().child("junction").is_err());
        std::fs::remove_dir(&junction).unwrap();
    }

    // This same test executable is launched in a separate OS process. The
    // environment points only at the unique fixture, never a real user store.
    #[test]
    fn cross_process_update_worker() {
        let Some(path) = std::env::var_os("XYZ_STORAGE_TEST_FIXTURE") else {
            return;
        };
        let path = Path::new(&path);
        assert!(path.starts_with(std::env::temp_dir()));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("xyz-store-test-"));
        assert!(path.is_dir());
        let directory = PrivateDirectory::open_or_create(path).unwrap();
        for _ in 0..12 {
            directory
                .update("counter", 32, |bytes| {
                    let value: u32 = std::str::from_utf8(bytes.unwrap())
                        .unwrap()
                        .parse()
                        .unwrap();
                    Ok((
                        (),
                        Some(Zeroizing::new((value + 1).to_string().into_bytes())),
                    ))
                })
                .unwrap();
        }
    }

    #[test]
    fn separate_processes_serialize_on_stable_lock_file() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        directory
            .update("counter", 32, |_| {
                Ok(((), Some(Zeroizing::new(b"0".to_vec()))))
            })
            .unwrap();
        let mut workers = Vec::new();
        for _ in 0..2 {
            workers.push(
                Command::new(std::env::current_exe().unwrap())
                    .arg("cross_process_update_worker")
                    .arg("--test-threads=1")
                    .env("XYZ_STORAGE_TEST_FIXTURE", &fixture.0)
                    .spawn()
                    .unwrap(),
            );
        }
        for mut worker in workers {
            assert!(worker.wait().unwrap().success());
        }
        assert_eq!(
            directory.read("counter", 32).unwrap().unwrap().as_slice(),
            b"24"
        );
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    }
}
