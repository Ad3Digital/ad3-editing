// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Bounded, owned font discovery. Metadata never requires mapping live files.
use crate::{Result, encoder::cancelled, media::SourceVersion};
use fontdb::{Database, ID, Source};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

pub(crate) fn read_font(
    path: &Path,
    cancel: Option<&AtomicBool>,
    budget: &mut u64,
) -> Result<(Vec<u8>, SourceVersion)> {
    cancelled(cancel)?;
    let before = SourceVersion::read(path)?;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("Font file exceeds 64 MiB".into());
    }
    let mut data = Vec::new();
    let mut chunk = [0u8; 16384];
    loop {
        cancelled(cancel)?;
        let available = budget.saturating_add(1).min(chunk.len() as u64) as usize;
        let count = file
            .read(&mut chunk[..available])
            .map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if data.len() + count > 64 * 1024 * 1024 {
            return Err("Font file exceeds 64 MiB".into());
        }
        if count as u64 > *budget {
            *budget = 0;
            return Err("Font I/O byte budget exceeded".into());
        }
        *budget -= count as u64;
        data.extend_from_slice(&chunk[..count]);
    }
    cancelled(cancel)?;
    if SourceVersion::read(path)? != before {
        return Err("Font changed while loading; reopen the project".into());
    }
    // fontdb reserves collection capacity from this header before parsing faces.
    if data.starts_with(b"ttcf")
        && data
            .get(8..12)
            .is_some_and(|n| u32::from_be_bytes(n.try_into().unwrap()) > 256)
    {
        return Err("Font collection exceeds 256 faces".into());
    }
    Ok((data, before))
}

fn system_dirs() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "windows")]
    {
        paths.push(
            PathBuf::from(std::env::var_os("SYSTEMROOT").unwrap_or_else(|| "C:/Windows".into()))
                .join("Fonts"),
        );
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            paths.push(PathBuf::from(&profile).join("AppData/Local/Microsoft/Windows/Fonts"));
            paths.push(PathBuf::from(profile).join("AppData/Roaming/Microsoft/Windows/Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        paths.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
        if let Some(user_dir) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(user_dir).join("Library/Fonts"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        paths.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        if let Some(user_dir) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(&user_dir).join(".fonts"));
            paths.push(PathBuf::from(user_dir).join(".local/share/fonts"));
        }
    }
    paths
}

pub(crate) fn discover(
    db: &mut Database,
    cancel: Option<&AtomicBool>,
) -> Result<HashMap<ID, SourceVersion>> {
    discover_dirs(db, system_dirs(), cancel, &mut (1024 * 1024 * 1024))
}
fn discover_dirs(
    db: &mut Database,
    dirs: Vec<PathBuf>,
    cancel: Option<&AtomicBool>,
    budget: &mut u64,
) -> Result<HashMap<ID, SourceVersion>> {
    let roots = dirs
        .into_iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<Vec<_>>();
    let mut pending = roots.clone();
    let mut seen_files = HashSet::new();
    let mut seen = HashSet::new();
    let mut versions = HashMap::new();
    let mut entries = 0usize;
    let mut files = 0usize;
    while let Some(path) = pending.pop() {
        cancelled(cancel)?;
        let Ok(path) = fs::canonicalize(path) else {
            continue;
        };
        if !roots.iter().any(|root| path.starts_with(root)) {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        if seen.len() > 16384 {
            return Err("Font discovery exceeds 16384 directories".into());
        }
        let Ok(directory) = fs::read_dir(&path) else {
            continue;
        };
        for entry in directory {
            cancelled(cancel)?;
            entries += 1;
            if entries > 65536 {
                return Err("Font discovery exceeds 65536 directory entries".into());
            }
            let Ok(entry) = entry else { continue };
            let Ok(path) = fs::canonicalize(entry.path()) else {
                continue;
            };
            if !roots.iter().any(|root| path.starts_with(root)) {
                continue;
            }
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            if !path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                ["ttf", "ttc", "otf", "otc"].contains(&s.to_ascii_lowercase().as_str())
            }) {
                continue;
            }
            if !seen_files.insert(path.clone()) {
                continue;
            }
            files += 1;
            if files > 8192 {
                return Err("Font discovery exceeds 8192 font files".into());
            }
            // Unreadable, oversized or malformed system fonts are unavailable,
            // like files ignored by fontdb's original directory discovery.
            let (bytes, version) = match read_font(&path, cancel, budget) {
                Ok(value) => value,
                Err(error) => {
                    cancelled(cancel)?;
                    if error == "Font I/O byte budget exceeded" {
                        return Err(error);
                    }
                    continue;
                }
            };
            let mut parsed = Database::new();
            parsed.load_font_data(bytes);
            for mut face in parsed.faces().cloned() {
                if db.len() >= 16384 {
                    return Err("Font discovery exceeds 16384 faces".into());
                }
                face.source = Source::File(version.path.clone());
                let id = db.push_face_info(face);
                versions.insert(id, version.clone());
            }
        }
    }
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_does_not_follow_a_link_outside_its_canonical_roots() {
        let base = std::env::temp_dir().join(format!(
            "ad3-font-link-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&base).unwrap();
        let allowed = base.join("allowed");
        let outside = base.join("outside");
        fs::create_dir(&allowed).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("escape.ttf"), [1u8; 32]).unwrap();
        let link = allowed.join("escape");
        #[cfg(windows)]
        {
            let status=crate::media::command(Path::new("powershell.exe")).args(["-NoProfile","-NonInteractive","-Command","New-Item -ItemType Junction -Path $env:AD3_FONT_TEST_LINK -Target $env:AD3_FONT_TEST_TARGET | Out-Null"])
                .env("AD3_FONT_TEST_LINK",&link).env("AD3_FONT_TEST_TARGET",&outside).status().unwrap();
            assert!(status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let mut db = Database::new();
        let mut budget = 0;
        assert!(
            discover_dirs(&mut db, vec![allowed.clone()], None, &mut budget)
                .unwrap()
                .is_empty()
        );
        assert!(db.is_empty());
        #[cfg(windows)]
        fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link).unwrap();
        fs::remove_file(outside.join("escape.ttf")).unwrap();
        fs::remove_dir(outside).unwrap();
        fs::remove_dir(allowed).unwrap();
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn font_reader_bounds_files_collections_and_checks_cancellation_before_io() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut budget = u64::MAX;
        let path = std::env::temp_dir().join(format!(
            "ad3-font-bound-{}-{}.ttc",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.set_len(64 * 1024 * 1024 + 1).unwrap();
        assert!(
            read_font(&path, None, &mut budget)
                .err()
                .unwrap()
                .contains("64 MiB")
        );
        file.set_len(0).unwrap();
        use std::io::Write;
        file.write_all(b"ttcf\0\x01\0\0\0\0\x01\x01").unwrap();
        drop(file);
        assert!(
            read_font(&path, None, &mut budget)
                .err()
                .unwrap()
                .contains("256 faces")
        );
        fs::remove_file(&path).unwrap();
        assert!(
            read_font(&path, Some(&AtomicBool::new(true)), &mut budget)
                .err()
                .unwrap()
                .contains("cancelled")
        );
    }
}
