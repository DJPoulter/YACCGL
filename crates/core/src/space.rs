use std::path::Path;

/// Free bytes on the filesystem that holds `path` (or its nearest existing ancestor).
pub fn available(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|p| p.exists())?;
    imp::available(existing)
}

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub fn available(path: &Path) -> Option<u64> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-param.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn available(_: &std::path::Path) -> Option<u64> {
        None
    }
}

/// "1.2 GB"-style formatting for the UI and CLI.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn human_sizes() {
        assert_eq!(super::human(512), "512 B");
        assert_eq!(super::human(340_386_629), "340.4 MB");
        assert_eq!(super::human(40_880_880_831), "40.9 GB");
    }

    #[cfg(unix)]
    #[test]
    fn available_on_missing_child_uses_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::available(&dir.path().join("does/not/exist")).is_some());
    }
}
