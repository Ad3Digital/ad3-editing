// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Source replacement retains the target's metadata, and a recoverable original
//! on Windows if ReplaceFileW cannot complete its rename/metadata merge.
use crate::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn source_target(root: &Path, target: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(target).map_err(|e| e.to_string())?;
    let package = target.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|name| {
            if cfg!(windows) {
                name.eq_ignore_ascii_case("node_modules")
            } else {
                name == "node_modules"
            }
        })
    });
    if !target.starts_with(root) || package {
        return Err(
            "Source save must stay inside the project and outside installed node_modules packages"
                .into(),
        );
    }
    Ok(target)
}

#[cfg(windows)]
pub(crate) fn replace(temp: &Path, target: &Path, expected: &[u8]) -> Result<()> {
    use std::{ffi::c_void, io, os::windows::ffi::OsStrExt, ptr};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
    }
    // Reserve a private empty directory, not an empty backup file. Therefore
    // an existing backup after the API call is always the moved original,
    // including a zero-byte original with named streams or other metadata.
    let backup_directory = temp.with_extension("backup-dir");
    fs::create_dir(&backup_directory).map_err(|e| e.to_string())?;
    let backup = backup_directory.join("original");
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let (target_wide, temp_wide, backup_wide) = (wide(target), wide(temp), wide(&backup));
    // SAFETY: all paths are live NUL-terminated UTF-16 buffers. Reserved
    // pointers are null and flags=0 requires successful metadata/ACL merging.
    let success = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            temp_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if success != 0 {
        // The displaced object is opened exclusively through comparison AND
        // deletion. A pre-existing writer (even one sharing DELETE) prevents
        // cleanup, so its later writes remain in the recoverable backup.
        discard_backup(&backup, expected).map_err(|error| format!(
            "Source replacement completed with a conflict: {error}. The displaced source is retained at {}. Reload and reconcile before editing again.",
            backup.display()
        ))?;
        if let Err(error) = fs::remove_dir(&backup_directory) {
            eprintln!(
                "Source saved; could not remove empty backup directory {}: {error}",
                backup_directory.display()
            );
        }
        return Ok(());
    }
    let error = io::Error::last_os_error();
    recover(target, &backup, &backup_directory, error)
}

#[cfg(windows)]
fn recover(target: &Path, backup: &Path, directory: &Path, error: std::io::Error) -> Result<()> {
    if !target.exists() && backup.is_file() {
        // Creating a hard link fails if another writer recreated the target.
        // A failed recovery retains the original backup regardless of length.
        if let Err(recovery) = fs::hard_link(backup, target) {
            return Err(format!(
                "Source replacement failed: {error}; original retained at {} (restore failed: {recovery})",
                backup.display()
            ));
        }
        // Retain the backup after ANY failure. Another writer may replace or
        // remove the just-restored target; it must not remove our last link.
        return Err(format!(
            "Source replacement failed: {error}; target name restored and original backup retained at {}",
            backup.display()
        ));
    } else if backup.exists() {
        return Err(format!(
            "Source replacement failed: {error}; uncertain original backup retained at {}",
            backup.display()
        ));
    } else if let Err(cleanup) = fs::remove_dir(directory) {
        eprintln!(
            "Source replacement failed; could not remove empty backup directory {}: {cleanup}",
            directory.display()
        );
    }
    Err(format!("Source replacement failed: {error}"))
}

#[cfg(windows)]
fn verified_backup(backup: &Path, expected: &[u8]) -> Result<fs::File> {
    use std::{io::Read, os::windows::fs::OpenOptionsExt};
    // GENERIC_READ | DELETE. Sharing=0 also rejects pre-existing handles that
    // could mutate the displaced file after validation. Keep this handle until
    // marking deletion; never close and then delete by pathname.
    let mut file = fs::OpenOptions::new()
        .access_mode(0x8000_0000 | 0x0001_0000)
        .share_mode(0)
        .open(backup)
        .map_err(|error| format!("could not acquire exclusive backup access: {error}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() != expected.len() as u64 {
        return Err("contents changed after the preflight check".into());
    }
    let mut actual = Vec::with_capacity(expected.len());
    file.read_to_end(&mut actual)
        .map_err(|error| format!("could not verify backup: {error}"))?;
    if actual != expected {
        return Err("contents changed after the preflight check".into());
    }
    Ok(file)
}

#[cfg(windows)]
fn discard_backup(backup: &Path, expected: &[u8]) -> Result<()> {
    use std::{ffi::c_void, io, os::windows::io::AsRawHandle};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetFileInformationByHandle(
            handle: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }
    let file = verified_backup(backup, expected)?;
    // FILE_DISPOSITION_INFO contains BOOLEAN (u8), not BOOL (i32).
    let mut delete_file = 1_u8;
    // SAFETY: file owns a live exclusive handle with DELETE access. Class 4 is
    // FileDispositionInfo and the one-byte buffer remains live for the call.
    let success = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            4,
            (&mut delete_file as *mut u8).cast(),
            1,
        )
    };
    if success == 0 {
        return Err(format!(
            "could not discard verified backup: {}",
            io::Error::last_os_error()
        ));
    }
    drop(file); // The same exclusively held object is now deleted by the OS.
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn replace(temp: &Path, target: &Path, expected: &[u8]) -> Result<()> {
    if fs::read(target).map_err(|e| e.to_string())? != expected {
        return Err("Source changed before replacement; reload before editing again".into());
    }
    let original = fs::metadata(target).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let replacement = fs::metadata(temp).map_err(|e| e.to_string())?;
        if original.uid() != replacement.uid() || original.gid() != replacement.gid() {
            return Err(
                "Source ownership differs from the new file; refusing to change it during save"
                    .into(),
            );
        }
    }
    fs::set_permissions(temp, original.permissions()).map_err(|e| e.to_string())?;
    fs::rename(temp, target).map_err(|e| e.to_string())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        io,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    fn directory() -> PathBuf {
        // Windows clock resolution can give parallel tests the same timestamp.
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ad3-rust-backup-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn failed_replace_restores_zero_byte_original_with_its_streams() {
        let folder = directory();
        let target = folder.join("source.tsx");
        let backup_directory = folder.join("backup-dir");
        fs::create_dir(&backup_directory).unwrap();
        let backup = backup_directory.join("original");
        fs::write(&backup, "").unwrap();
        fs::write(format!("{}:metadata", backup.display()), "retain metadata").unwrap();
        assert!(
            recover(
                &target,
                &backup,
                &backup_directory,
                io::Error::from_raw_os_error(1177)
            )
            .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"");
        assert_eq!(
            fs::read_to_string(format!("{}:metadata", target.display())).unwrap(),
            "retain metadata"
        );
        assert!(backup.exists());
        // Even if an outside editor removes the restored name, the original
        // and its streams remain recoverable through the retained backup.
        fs::remove_file(target).unwrap();
        assert_eq!(
            fs::read_to_string(format!("{}:metadata", backup.display())).unwrap(),
            "retain metadata"
        );
        fs::remove_file(backup).unwrap();
        fs::remove_dir(backup_directory).unwrap();
        fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn uncertain_replace_keeps_both_files_instead_of_removing_a_backup() {
        let folder = directory();
        let target = folder.join("source.tsx");
        let backup_directory = folder.join("backup-dir");
        fs::create_dir(&backup_directory).unwrap();
        let backup = backup_directory.join("original");
        fs::write(&target, "another writer").unwrap();
        fs::write(&backup, "").unwrap();
        let error = recover(
            &target,
            &backup,
            &backup_directory,
            io::Error::from_raw_os_error(1177),
        )
        .unwrap_err();
        assert!(error.contains("retained"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "another writer");
        assert!(backup.exists());
        fs::remove_file(target).unwrap();
        fs::remove_file(backup).unwrap();
        fs::remove_dir(backup_directory).unwrap();
        fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn locked_target_fails_without_removing_the_original_or_candidate() {
        use std::os::windows::fs::OpenOptionsExt;
        let folder = directory();
        let target = folder.join("source.tsx");
        let temp = folder.join("candidate.tmp");
        fs::write(&target, "original").unwrap();
        fs::write(&temp, "candidate").unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .unwrap();
        assert!(replace(&temp, &target, b"original").is_err());
        drop(locked);
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        assert_eq!(fs::read_to_string(&temp).unwrap(), "candidate");
        assert!(!temp.with_extension("backup-dir").exists());
        fs::remove_file(target).unwrap();
        fs::remove_file(temp).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn concurrent_external_edit_is_retained_after_successful_replacement() {
        let folder = directory();
        let target = folder.join("source.tsx");
        let temp = folder.join("candidate.tmp");
        let expected = b"original before preflight";
        // Simulate an external edit after the writer's preflight comparison.
        fs::write(&target, "external edit after preflight").unwrap();
        fs::write(&temp, "native candidate").unwrap();
        let error = replace(&temp, &target, expected).unwrap_err();
        let backup_directory = temp.with_extension("backup-dir");
        let backup = backup_directory.join("original");
        assert!(error.contains("conflict"));
        assert!(error.contains(&backup.display().to_string()));
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "external edit after preflight"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "native candidate");
        fs::remove_file(target).unwrap();
        fs::remove_file(backup).unwrap();
        fs::remove_dir(backup_directory).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn open_external_writer_keeps_its_displaced_file_and_later_writes() {
        use std::{io::Write, os::windows::fs::OpenOptionsExt};
        let folder = directory();
        let target = folder.join("source.tsx");
        let temp = folder.join("candidate.tmp");
        fs::write(&target, "original").unwrap();
        fs::write(&temp, "candidate").unwrap();
        let mut external = fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&target)
            .unwrap();
        let error = replace(&temp, &target, b"original").unwrap_err();
        assert!(error.contains("exclusive backup access"));
        // The outside editor continues writing through its old handle, after
        // replacement and validation attempt. Those bytes must survive close.
        external.write_all(b"late external write").unwrap();
        external.sync_all().unwrap();
        drop(external);
        let backup_directory = temp.with_extension("backup-dir");
        let backup = backup_directory.join("original");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "late external write");
        assert_eq!(fs::read_to_string(&target).unwrap(), "candidate");
        fs::remove_file(target).unwrap();
        fs::remove_file(backup).unwrap();
        fs::remove_dir(backup_directory).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn verified_backup_handle_prevents_mutation_or_rename_before_deletion() {
        let folder = directory();
        let backup = folder.join("original");
        fs::write(&backup, "original").unwrap();
        let exclusive = verified_backup(&backup, b"original").unwrap();
        assert!(fs::write(&backup, "late mutation").is_err());
        assert!(fs::rename(&backup, folder.join("moved")).is_err());
        assert!(fs::remove_file(&backup).is_err());
        drop(exclusive);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
        discard_backup(&backup, b"original").unwrap();
        assert!(!backup.exists());
        fs::remove_dir(folder).unwrap();
    }
}
