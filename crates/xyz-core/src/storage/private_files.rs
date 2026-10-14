//! Private, bounded and handle-relative file operations. Lock files are stable:
//! never renamed/deleted by this code, independent of atomic data-file replace.
#[cfg(target_os = "linux")]
#[path = "private_files_linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "private_files_windows.rs"]
mod platform;
#[cfg(any(target_os = "linux", windows))]
pub(crate) use platform::PrivateDirectory;

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::super::{Result, StorageError};
    use std::path::Path;
    use zeroize::Zeroizing;
    pub(crate) struct PrivateDirectory;
    impl PrivateDirectory {
        pub fn open_or_create(_: &Path) -> Result<Self> {
            Err(StorageError::Unavailable)
        }
        pub fn child(&self, _: &str) -> Result<Self> {
            Err(StorageError::Unavailable)
        }
        pub fn prepare_database_file(&self, _: &str, _: usize) -> Result<bool> {
            Err(StorageError::Unavailable)
        }
        pub fn validate_database_file(&self, _: &str, _: usize) -> Result<bool> {
            Err(StorageError::Unavailable)
        }
        pub fn read(&self, _: &str, _: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
            Err(StorageError::Unavailable)
        }
        pub fn update<T>(
            &self,
            _: &str,
            _: usize,
            _: impl FnOnce(Option<&[u8]>) -> Result<(T, Option<Zeroizing<Vec<u8>>>)>,
        ) -> Result<T> {
            Err(StorageError::Unavailable)
        }
    }
}
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) use platform::PrivateDirectory;
