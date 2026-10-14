//! Read a private local file without reopening a checked pathname.
//!
//! Every component after the drive root is opened relative to its parent's
//! handle. Reparse points, remote volumes, alternate streams, hard links, and
//! permissive/unsupported ACLs fail closed. The immediate parent and file must
//! belong to the effective user; their DACLs may grant only that user, SYSTEM,
//! and Builtin Administrators. No ACL or privilege is changed by this module.
//!
//! This boundary cannot protect against administrators, a compromised process
//! running as the same user, or a malicious filesystem driver. The caller must
//! protect/zeroize returned secret bytes when they are no longer needed.
//!
//! API contracts: Microsoft Learn, NtCreateFile / OBJECT_ATTRIBUTES,
//! GetSecurityInfo, and MS-FSCC FileFsDeviceInformation. In particular,
//! FILE_FLAG_OPEN_REPARSE_POINT on just the final pathname is insufficient.

use std::{
    ffi::c_void,
    fs::File,
    io::Read,
    mem::{offset_of, size_of},
    os::windows::{ffi::OsStrExt, io::AsRawHandle, io::FromRawHandle, io::OwnedHandle},
    path::{Component, Path, Prefix},
    ptr::{null, null_mut},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileFsDeviceInformation, NtCreateFile, NtQueryVolumeInformationFile,
            FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT,
        },
        System::SystemServices::{
            FILE_FS_DEVICE_INFORMATION, FILE_REMOTE_DEVICE, FILE_REMOTE_DEVICE_VSMB,
        },
    },
    Win32::{
        Foundation::{
            GetLastError, LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, ERROR_SUCCESS,
            HANDLE, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, UNICODE_STRING,
        },
        Security::{
            Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
            EqualSid, GetAce, GetLengthSid, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
            GetTokenInformation, IsValidAcl, IsValidSecurityDescriptor, IsValidSid, IsWellKnownSid,
            TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE,
            ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, GetFileType, GetVolumeInformationByHandleW,
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DEVICE_DISK, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK,
            OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE,
        },
        System::{
            SystemServices::{
                ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, FILE_PERSISTENT_ACLS,
            },
            Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
            IO::IO_STATUS_BLOCK,
        },
    },
};
use zeroize::Zeroize;

type SafeResult<T> = Result<T, &'static str>;
const INVALID_PATH: &str = "PRIVATE_PATH_INVALID";
const OPEN_FAILED: &str = "PRIVATE_FILE_OPEN_FAILED";
const UNSAFE_FILE: &str = "PRIVATE_FILE_UNSAFE";
const UNSAFE_PARENT: &str = "PRIVATE_PARENT_UNSAFE";
const TOO_LARGE: &str = "PRIVATE_FILE_TOO_LARGE";
const READ_FAILED: &str = "PRIVATE_FILE_READ_FAILED";

/// Read at most `max_bytes` from an owned, private, single-link local disk file.
/// Errors are fixed codes and never contain paths, OS error strings, or data.
pub fn read_private_file(path: &Path, max_bytes: usize) -> SafeResult<Vec<u8>> {
    let (root_name, names) = checked_components(path)?;
    let identity = Identity::effective()?;
    let root = open_drive_root(&root_name)?;
    validate_directory(&root)?;
    let mut directories = vec![root];
    for name in &names[..names.len() - 1] {
        let directory = open_child(directories.last().unwrap(), name, true)?;
        validate_directory(&directory)?;
        directories.push(directory);
    }
    let parent = directories.last().unwrap();
    validate_private_acl(parent.as_raw_handle(), identity.sid()).map_err(|_| UNSAFE_PARENT)?;

    let handle = open_child(parent, names.last().unwrap(), false)?;
    let mut file = File::from(handle);
    validate_file(&file, max_bytes)?;
    validate_private_acl(file.as_raw_handle(), identity.sid())?;

    // No FILE_SHARE_WRITE/DELETE on this same handle. A writer already holding
    // an incompatible handle makes the open fail instead of weakening policy.
    let limit = u64::try_from(max_bytes)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or(TOO_LARGE)?;
    let mut bytes = Vec::new();
    let result = (|| {
        (&mut file)
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|_| READ_FAILED)?;
        if bytes.len() > max_bytes {
            return Err(TOO_LARGE);
        }
        // Recheck the same objects, never the pathname. Ownership/ACL changes
        // are not governed by data sharing modes; fail closed if one is seen.
        validate_file(&file, max_bytes)?;
        validate_private_acl(file.as_raw_handle(), identity.sid())?;
        validate_private_acl(parent.as_raw_handle(), identity.sid()).map_err(|_| UNSAFE_PARENT)?;
        Ok(())
    })();
    if let Err(error) = result {
        bytes.zeroize();
        return Err(error);
    }
    Ok(bytes)
}

pub(crate) fn checked_components(path: &Path) -> SafeResult<(Vec<u16>, Vec<Vec<u16>>)> {
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) if drive.is_ascii_alphabetic() => drive,
            _ => return Err(INVALID_PATH),
        },
        _ => return Err(INVALID_PATH),
    };
    if components.next() != Some(Component::RootDir)
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(INVALID_PATH);
    }
    // Path::components normalizes interior '.', so also validate the original
    // UTF-16 segments. Do not allow Win32 aliases or alternate data streams.
    let raw: Vec<_> = path.as_os_str().encode_wide().collect();
    if raw.len() < 4 || raw.len() > 4096 {
        return Err(INVALID_PATH);
    }
    let mut names = Vec::new();
    for name in raw[3..].split(|unit| *unit == b'\\' as u16 || *unit == b'/' as u16) {
        if name.is_empty()
            || name.len() > 255
            || matches!(name.last(), Some(32 | 46))
            || name
                .iter()
                .any(|unit| *unit < 32 || (*b":*?\"<>|").map(u16::from).contains(unit))
            || reserved_name(name)
        {
            return Err(INVALID_PATH);
        }
        names.push(name.to_vec());
    }
    if names.is_empty() {
        return Err(INVALID_PATH);
    }
    Ok((vec![drive.into(), b':'.into(), b'\\'.into(), 0], names))
}

fn reserved_name(name: &[u16]) -> bool {
    let stem = name.split(|unit| *unit == b'.' as u16).next().unwrap();
    let upper: Vec<_> = stem
        .iter()
        .map(|unit| match u8::try_from(*unit) {
            Ok(byte) => u16::from(byte.to_ascii_uppercase()),
            Err(_) => *unit,
        })
        .collect();
    let matches = |value: &str| upper.iter().copied().eq(value.encode_utf16());
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"]
        .iter()
        .any(|value| matches(value))
        || (upper.len() == 4
            && (upper[..3] == [b'C' as u16, b'O' as u16, b'M' as u16]
                || upper[..3] == [b'L' as u16, b'P' as u16, b'T' as u16])
            && (matches!(upper[3], 0x31..=0x39 | 0xb9 | 0xb2 | 0xb3)))
}

pub(crate) fn open_drive_root(name: &[u16]) -> SafeResult<OwnedHandle> {
    // SAFETY: checked_components supplies a terminated drive root. No
    // SECURITY_ATTRIBUTES means no inherited handle or changed descriptor.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_TRAVERSE | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    owned_handle(handle)
}

pub(crate) fn owned_handle(handle: HANDLE) -> SafeResult<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(OPEN_FAILED)
    } else {
        // SAFETY: successful APIs transfer one owned, non-inherited handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn open_child(parent: &OwnedHandle, name: &[u16], directory: bool) -> SafeResult<OwnedHandle> {
    let mut wide = name.to_vec();
    let length = u16::try_from(wide.len() * size_of::<u16>()).map_err(|_| INVALID_PATH)?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        ..Default::default()
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = null_mut();
    let (access, sharing, kind) = if directory {
        (
            FILE_READ_ATTRIBUTES | FILE_TRAVERSE | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_DIRECTORY_FILE,
        )
    } else {
        (FILE_GENERIC_READ, FILE_SHARE_READ, FILE_NON_DIRECTORY_FILE)
    };
    // SAFETY: all structures and UTF-16 storage outlive this synchronous call;
    // each name is one ordinary component relative to a live directory handle.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            null(),
            0,
            sharing,
            FILE_OPEN,
            kind | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            null(),
            0,
        )
    };
    if result < 0 {
        return Err(OPEN_FAILED);
    }
    owned_handle(handle)
}

pub(crate) fn file_information(handle: HANDLE) -> SafeResult<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: handle is live and output points to the complete writable struct.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(UNSAFE_FILE);
    }
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE) != 0 {
        return Err(UNSAFE_FILE);
    }
    Ok(info)
}

pub(crate) fn validate_directory(handle: &OwnedHandle) -> SafeResult<()> {
    let info = file_information(handle.as_raw_handle())?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(UNSAFE_FILE);
    }
    validate_volume(handle.as_raw_handle())
}

pub(crate) fn validate_file(file: &File, max_bytes: usize) -> SafeResult<()> {
    let info = file_information(file.as_raw_handle())?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 || info.nNumberOfLinks != 1 {
        return Err(UNSAFE_FILE);
    }
    let length = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    if length > max_bytes as u64 {
        return Err(TOO_LARGE);
    }
    validate_volume(file.as_raw_handle())
}

fn local_device(info: &FILE_FS_DEVICE_INFORMATION) -> bool {
    info.DeviceType == FILE_DEVICE_DISK
        && info.Characteristics & (FILE_REMOTE_DEVICE | FILE_REMOTE_DEVICE_VSMB) == 0
}

fn validate_volume(handle: HANDLE) -> SafeResult<()> {
    let mut info = FILE_FS_DEVICE_INFORMATION::default();
    let mut status = IO_STATUS_BLOCK::default();
    let mut flags = 0;
    // SAFETY: the live synchronous handle and correctly sized output buffers
    // remain valid. Queries are on the opened volume, not a drive-name guess.
    let valid = unsafe {
        GetFileType(handle) == FILE_TYPE_DISK
            && NtQueryVolumeInformationFile(
                handle,
                &mut status,
                (&mut info as *mut FILE_FS_DEVICE_INFORMATION).cast(),
                size_of::<FILE_FS_DEVICE_INFORMATION>() as u32,
                FileFsDeviceInformation,
            ) >= 0
            && local_device(&info)
            && GetVolumeInformationByHandleW(
                handle,
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                &mut flags,
                null_mut(),
                0,
            ) != 0
            && flags & FILE_PERSISTENT_ACLS != 0
    };
    if valid {
        Ok(())
    } else {
        Err(UNSAFE_FILE)
    }
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: this allocation was returned by an API requiring LocalFree.
        unsafe { LocalFree(self.0) };
    }
}

pub(crate) struct Identity(Vec<usize>);
impl Identity {
    pub(crate) fn effective() -> SafeResult<Self> {
        let mut token = null_mut();
        // SAFETY: pseudo-handles are valid for these token queries; the output
        // transfers an owned token handle. Never modify privileges or identity.
        unsafe {
            if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0
                && (GetLastError() != ERROR_NO_TOKEN
                    || OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0)
            {
                return Err(UNSAFE_FILE);
            }
        }
        let token = owned_handle(token)?;
        let mut length = 0;
        // SAFETY: null output with zero capacity requests only the needed size.
        unsafe {
            if GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length)
                != 0
                || GetLastError() != ERROR_INSUFFICIENT_BUFFER
            {
                return Err(UNSAFE_FILE);
            }
        }
        if length < size_of::<TOKEN_USER>() as u32 || length > 64 * 1024 {
            return Err(UNSAFE_FILE);
        }
        let mut data = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        // SAFETY: usize storage provides TOKEN_USER alignment and at least the
        // requested capacity. Windows returns the SID inside this buffer.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                data.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(UNSAFE_FILE);
        }
        let identity = Self(data);
        // SAFETY: GetTokenInformation initialized a complete TOKEN_USER/SID.
        if unsafe { IsValidSid(identity.sid()) } == 0 {
            return Err(UNSAFE_FILE);
        }
        Ok(identity)
    }

    pub(crate) fn sid(&self) -> PSID {
        // SAFETY: only effective() constructs Identity after TokenUser succeeds;
        // its aligned allocation remains live and at the same address.
        unsafe { (*(self.0.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }
}

pub(crate) fn validate_private_acl(handle: HANDLE, user: PSID) -> SafeResult<()> {
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the handle has READ_CONTROL; returned owner/ACL pointers live in
    // the descriptor, whose allocation is retained until validation completes.
    let result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != ERROR_SUCCESS || descriptor.is_null() {
        return Err(UNSAFE_FILE);
    }
    let _allocation = LocalAllocation(descriptor);
    validate_descriptor(descriptor, user)
}

fn validate_descriptor(descriptor: PSECURITY_DESCRIPTOR, user: PSID) -> SafeResult<()> {
    let mut owner = null_mut();
    let mut defaulted = 0;
    // SAFETY: descriptor comes from a successful Windows security query or
    // SDDL conversion and remains allocated throughout this validation.
    if unsafe {
        IsValidSecurityDescriptor(descriptor) == 0
            || GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
            || owner.is_null()
            || IsValidSid(owner) == 0
            || EqualSid(owner, user) == 0
    } {
        return Err(UNSAFE_FILE);
    }
    let mut present = 0;
    let mut acl = null_mut();
    // SAFETY: descriptor was validated; outputs have the declared sizes.
    if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) } == 0
        || present == 0
        || acl.is_null()
    {
        return Err(UNSAFE_FILE);
    }
    validate_acl(acl, user)
}

fn validate_acl(acl: *const ACL, user: PSID) -> SafeResult<()> {
    // SAFETY: caller passes a DACL from a live Windows security descriptor.
    if unsafe { IsValidAcl(acl) } == 0 {
        return Err(UNSAFE_FILE);
    }
    // SAFETY: IsValidAcl verified the ACL header/entries. Extra bounds below
    // ensure an ACE's SID fits before invoking SID APIs.
    let header = unsafe { &*acl };
    let start = acl as usize;
    let end = start
        .checked_add(header.AclSize as usize)
        .ok_or(UNSAFE_FILE)?;
    for index in 0..u32::from(header.AceCount) {
        let mut ace = null_mut();
        // SAFETY: index is bounded by the validated ACL's entry count.
        if unsafe { GetAce(acl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(UNSAFE_FILE);
        }
        let at = ace as usize;
        if at < start + size_of::<ACL>()
            || at
                .checked_add(size_of::<ACE_HEADER>())
                .is_none_or(|n| n > end)
        {
            return Err(UNSAFE_FILE);
        }
        // SAFETY: the ACE_HEADER range was checked and Windows aligns ACEs.
        let entry = unsafe { &*ace.cast::<ACE_HEADER>() };
        let ace_end = at.checked_add(entry.AceSize as usize).ok_or(UNSAFE_FILE)?;
        if ace_end > end || entry.AceFlags & !0x1f != 0 {
            return Err(UNSAFE_FILE);
        }
        // Only ordinary allow/deny entries have understood semantics here.
        // Callback, object-specific, compound, and future ACEs fail closed.
        if !matches!(
            u32::from(entry.AceType),
            ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE
        ) {
            return Err(UNSAFE_FILE);
        }
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if usize::from(entry.AceSize) < sid_offset + 8 {
            return Err(UNSAFE_FILE);
        }
        // SAFETY: a SID has an 8-byte header; SubAuthorityCount is its second
        // byte. Bound all subauthorities before IsValidSid/GetLengthSid.
        let sid = unsafe { ace.cast::<u8>().add(sid_offset) };
        let count = unsafe { *sid.add(1) } as usize;
        if sid_offset + 8 + count * 4 > usize::from(entry.AceSize)
            || unsafe { IsValidSid(sid.cast()) } == 0
            || unsafe { GetLengthSid(sid.cast()) } as usize != 8 + count * 4
        {
            return Err(UNSAFE_FILE);
        }
        // Denies only remove access. Never subtract a deny to justify a broad
        // allow: noncanonical ordering/conditional semantics cannot fool us.
        if u32::from(entry.AceType) == ACCESS_ALLOWED_ACE_TYPE {
            // SAFETY: the basic ACE layout and complete SID were checked.
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            if allowed.Mask != 0
                && unsafe {
                    EqualSid(sid.cast(), user) == 0
                        && IsWellKnownSid(sid.cast(), WinLocalSystemSid) == 0
                        && IsWellKnownSid(sid.cast(), WinBuiltinAdministratorsSid) == 0
                }
            {
                return Err(UNSAFE_FILE);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr,
        io::Write,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    use windows_sys::Win32::{
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateDirectoryW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE,
        },
        System::{
            Ioctl::FSCTL_SET_REPARSE_POINT, SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
            IO::DeviceIoControl,
        },
    };

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    fn user_sid() -> String {
        let identity = Identity::effective().unwrap();
        let mut text = null_mut();
        // SAFETY: valid SID; API returns a terminated LocalAlloc string.
        assert_ne!(
            unsafe { ConvertSidToStringSidW(identity.sid(), &mut text) },
            0
        );
        let _allocation = LocalAllocation(text.cast());
        let mut length = 0;
        // SAFETY: successful conversion returns a NUL-terminated UTF-16 string.
        unsafe {
            while *text.add(length) != 0 {
                length += 1;
            }
            String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap()
        }
    }

    fn descriptor(sddl: &str) -> LocalAllocation {
        let text = wide(OsStr::new(sddl));
        let mut value = null_mut();
        // SAFETY: terminated SDDL and writable output; returned allocation is owned.
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut value,
                    null_mut(),
                )
            },
            0
        );
        LocalAllocation(value)
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(broad_parent: bool) -> Self {
            let path = std::env::temp_dir().join(format!(
                "xyz-private-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let sid = user_sid();
            let broad = if broad_parent { "(A;OICI;FR;;;WD)" } else { "" };
            let sd = descriptor(&format!(
                "O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){broad}"
            ));
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: 0,
            };
            let name = wide(path.as_os_str());
            // SAFETY: create only a unique disposable fixture directory.
            assert_ne!(unsafe { CreateDirectoryW(name.as_ptr(), &attributes) }, 0);
            Self(path)
        }

        fn file(&self, name: &str, extra_ace: &str, contents: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            let sid = user_sid();
            let sd = descriptor(&format!(
                "O:{sid}D:P(A;;FA;;;{sid})(A;;FA;;;SY)(A;;FA;;;BA){extra_ace}"
            ));
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: 0,
            };
            let name = wide(path.as_os_str());
            // SAFETY: create only a new fixture file, with its explicit test ACL.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    FILE_GENERIC_WRITE,
                    0,
                    &attributes,
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            let mut file = File::from(owned_handle(handle).unwrap());
            file.write_all(contents).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn private_file_accepts_owner_system_admin_and_enforces_size() {
        let fixture = Fixture::new(false);
        let path = fixture.file("密钥 config", "", b"dummy-secret");
        assert_eq!(read_private_file(&path, 12).unwrap(), b"dummy-secret");
        assert_eq!(read_private_file(&path, 11), Err(TOO_LARGE));
    }

    #[test]
    fn broad_file_and_parent_acl_are_rejected() {
        let fixture = Fixture::new(false);
        for (name, principal) in [("everyone", "WD"), ("users", "BU"), ("authenticated", "AU")] {
            let path = fixture.file(name, &format!("(A;;FR;;;{principal})"), b"dummy");
            assert_eq!(read_private_file(&path, 32), Err(UNSAFE_FILE));
        }
        let broad = Fixture::new(true);
        let path = broad.file("private-leaf", "", b"dummy");
        assert_eq!(read_private_file(&path, 32), Err(UNSAFE_PARENT));
    }

    #[test]
    fn rejects_hard_links_and_directories() {
        let fixture = Fixture::new(false);
        let path = fixture.file("key", "", b"dummy");
        std::fs::hard_link(&path, fixture.0.join("alias")).unwrap();
        assert_eq!(read_private_file(&path, 32), Err(UNSAFE_FILE));
        assert!(read_private_file(&fixture.0, 32).is_err());
    }

    #[test]
    fn refuses_an_existing_writer_without_reopening_or_weakening_sharing() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new(false);
        let path = fixture.file("key", "", b"dummy");
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&path)
            .unwrap();
        assert_eq!(read_private_file(&path, 32), Err(OPEN_FAILED));
        drop(writer);
        assert_eq!(read_private_file(&path, 32).unwrap(), b"dummy");
    }

    #[test]
    fn rejects_path_aliases_and_remote_names_before_open() {
        for name in [
            r"C:relative",
            r"\rooted",
            r"relative",
            r"\\server\share\key",
            r"\\?\C:\key",
            r"\\.\C:\key",
            r"C:\x\..\key",
            r"C:\x\.\key",
            r"C:\x\key:stream",
            r"C:\x\NUL.txt",
            r"C:\x\COM1",
            r"C:\x\LPT²",
            r"C:\x\key.",
            "C:\\x\\key ",
            "C:\\x\\key\0",
            r"C:\x\\key",
            r"C:\x\",
        ] {
            assert!(
                checked_components(Path::new(name)).is_err(),
                "accepted path alias"
            );
        }
        assert!(checked_components(Path::new(r"C:\用户\keys\秘密")).is_ok());
    }

    #[test]
    fn actual_remote_device_is_rejected_even_with_disk_type() {
        let mut info = FILE_FS_DEVICE_INFORMATION {
            DeviceType: FILE_DEVICE_DISK,
            Characteristics: 0,
        };
        assert!(local_device(&info));
        info.Characteristics = FILE_REMOTE_DEVICE;
        assert!(!local_device(&info));
        info.Characteristics = FILE_REMOTE_DEVICE_VSMB;
        assert!(!local_device(&info));
        info.Characteristics = 0;
        info.DeviceType = 0x11; // Named pipe, not an ordinary disk volume.
        assert!(!local_device(&info));
    }

    #[test]
    fn null_dacl_missing_owner_and_foreign_owner_are_rejected() {
        let identity = Identity::effective().unwrap();
        let sid = user_sid();
        for value in [
            format!("O:{sid}D:NO_ACCESS_CONTROL"),
            format!("D:P(A;;FA;;;{sid})"),
            format!("O:WDD:P(A;;FA;;;{sid})"),
            format!("O:{sid}"),
        ] {
            let sd = descriptor(&value);
            assert_eq!(validate_descriptor(sd.0, identity.sid()), Err(UNSAFE_FILE));
        }
    }

    #[test]
    fn inheritance_and_deny_entries_do_not_excuse_broad_grants() {
        let identity = Identity::effective().unwrap();
        let sid = user_sid();
        for grant in [
            "(A;ID;FR;;;WD)",
            "(A;OIIO;FR;;;WD)",
            "(D;;FR;;;WD)(A;;FR;;;WD)",
        ] {
            let sd = descriptor(&format!("O:{sid}D:P(A;;FA;;;{sid}){grant}"));
            assert_eq!(validate_descriptor(sd.0, identity.sid()), Err(UNSAFE_FILE));
        }
        let sd = descriptor(&format!("O:{sid}D:P(D;;FW;;;WD)(A;;FR;;;{sid})"));
        assert!(validate_descriptor(sd.0, identity.sid()).is_ok());
    }

    #[test]
    fn unknown_ace_semantics_fail_closed() {
        let identity = Identity::effective().unwrap();
        let sd = descriptor(&format!("O:{}D:P(A;;FA;;;{})", user_sid(), user_sid()));
        let (mut present, mut defaulted, mut acl, mut ace) = (0, 0, null_mut(), null_mut());
        // SAFETY: locally constructed descriptor remains live; modify only the
        // first ACE's type in this disposable test allocation.
        unsafe {
            assert_ne!(
                GetSecurityDescriptorDacl(sd.0, &mut present, &mut acl, &mut defaulted),
                0
            );
            assert_ne!(GetAce(acl, 0, &mut ace), 0);
            (*ace.cast::<ACE_HEADER>()).AceType = 0x7f;
        }
        assert_eq!(validate_descriptor(sd.0, identity.sid()), Err(UNSAFE_FILE));
    }

    #[test]
    fn rejects_junction_in_intermediate_path() {
        let fixture = Fixture::new(false);
        fixture.file("key", "", b"dummy");
        let link = fixture.0.join("junction");
        std::fs::create_dir(&link).unwrap();
        let name = wide(link.as_os_str());
        // SAFETY: open only the unique disposable junction fixture itself.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let handle = owned_handle(handle).unwrap();
        let substitute = wide(OsStr::new(&format!(r"\??\{}", fixture.0.display())));
        let print = wide(fixture.0.as_os_str());
        // REPARSE_DATA_BUFFER mount-point header: lengths/offsets are bytes,
        // exclude terminators from each name length, include them in payload.
        let payload_bytes = 8 + (substitute.len() + print.len()) * 2;
        let mut buffer = Vec::new();
        buffer.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        for value in [
            payload_bytes as u16,
            0,
            0,
            ((substitute.len() - 1) * 2) as u16,
            (substitute.len() * 2) as u16,
            ((print.len() - 1) * 2) as u16,
        ] {
            buffer.extend(value.to_le_bytes());
        }
        for unit in substitute.into_iter().chain(print) {
            buffer.extend(unit.to_le_bytes());
        }
        let mut returned = 0;
        // SAFETY: buffer is a complete length-checked mount-point structure and
        // the target handle is our disposable empty test directory. No device,
        // privileges, existing ACL, or system settings are modified.
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    handle.as_raw_handle(),
                    FSCTL_SET_REPARSE_POINT,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    null_mut(),
                    0,
                    &mut returned,
                    null_mut(),
                )
            },
            0,
            "unable to create disposable junction fixture"
        );
        drop(handle);
        assert!(read_private_file(&link.join("key"), 32).is_err());
        // Remove the junction itself before fixture cleanup, never its target.
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn rejects_symlink_when_creation_privilege_is_available() {
        // Runtime link fixtures require privileges on some Windows images.
        // The mandatory junction test above also exercises traversal.
        let fixture = Fixture::new(false);
        let path = fixture.file("key", "", b"dummy");
        let link = fixture.0.join("link");
        match std::os::windows::fs::symlink_file(&path, &link) {
            Ok(()) => assert!(read_private_file(&link, 32).is_err()),
            Err(error) if error.raw_os_error() == Some(1314) => {
                // No symlink privilege: directory junction test remains mandatory.
            }
            Err(error) => panic!("unable to create disposable symlink fixture: {error}"),
        }
    }
}
