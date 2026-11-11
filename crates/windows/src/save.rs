//! Windows replacement preserves the original file's DACL and alternate streams.
use std::{
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
unsafe extern "system" {
    fn ReplaceFileW(
        replaced: *const u16,
        replacement: *const u16,
        backup: *const u16,
        flags: u32,
        exclude: *const std::ffi::c_void,
        reserved: *const std::ffi::c_void,
    ) -> i32;
}
pub fn checked(
    root: &Path,
    path: &Path,
    expected: &str,
    snapshot: &rivet_core::editor::Snapshot,
) -> io::Result<PathBuf> {
    rivet_core::documents::save_with_replace(root, path, expected, snapshot, |temporary, target| {
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        let temporary: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: both paths are owned NUL-terminated UTF-16; optional parameters are null.
        if unsafe {
            ReplaceFileW(
                target.as_ptr(),
                temporary.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_only_expected_workspace_file() {
        let root = std::env::temp_dir().join(format!("rivet-win-save-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("source.rs");
        std::fs::write(&file, "original").unwrap();
        let mut b = rivet_core::editor::Buffer::new(std::sync::Arc::from("original"));
        b.replace(0..8, "saved 🦀").unwrap();
        checked(&root, &file, "original", &b.snapshot()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "saved 🦀");
        assert!(checked(&root, &file, "original", &b.snapshot()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
