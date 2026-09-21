//! 7z extraction with progress, cancellation and path sanitising.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sevenz_rust2::{ArchiveReader, Password};

use crate::error::IoContext;
use crate::{Error, Result};

/// Extract `archive` into `dest`, overwriting existing files.
/// `on(done, total)` reports uncompressed bytes written.
pub fn extract_7z(
    archive: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let file = File::open(archive).io_ctx(|| format!("opening {}", archive.display()))?;
    let mut reader = ArchiveReader::new(file, Password::empty())
        .map_err(|e| Error::Extract(format!("{}: {e}", archive.display())))?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    reader.set_thread_count(threads.min(8));

    let total: u64 = reader.archive().files.iter().map(|f| f.size()).sum();
    fs::create_dir_all(dest).io_ctx(|| format!("creating {}", dest.display()))?;

    let mut done = 0u64;
    let mut failure: Option<Error> = None;
    let mut buf = vec![0u8; 256 * 1024];
    on(0, total);
    let result = reader.for_each_entries(|entry, data| {
        let step = (|| -> Result<()> {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let Some(rel) = sanitize(entry.name()) else {
                return Err(Error::Extract(format!("unsafe path in archive: {:?}", entry.name())));
            };
            let target = dest.join(&rel);
            if entry.is_directory() {
                return fs::create_dir_all(&target).io_ctx(|| format!("creating {}", target.display()));
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
            }
            // Write beside the target and rename, so an interrupted update never
            // leaves a half-written file under the real name.
            let tmp = tmp_path(&target);
            let mut out = File::create(&tmp).io_ctx(|| format!("creating {}", tmp.display()))?;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    drop(out);
                    let _ = fs::remove_file(&tmp);
                    return Err(Error::Cancelled);
                }
                let n = match data.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(Error::Extract(format!("{}: {e}", entry.name()))),
                };
                out.write_all(&buf[..n]).io_ctx(|| format!("writing {}", tmp.display()))?;
                done += n as u64;
                on(done, total);
            }
            drop(out);
            fs::rename(&tmp, &target).io_ctx(|| format!("replacing {}", target.display()))
        })();
        match step {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Err(sevenz_rust2::Error::Other("aborted".into()))
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    result.map_err(|e| Error::Extract(e.to_string()))
}

/// Turn an archive entry name into a safe relative path, or `None` if it would
/// escape the destination. Archives built on Windows may use `\` separators.
fn sanitize(name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut out = PathBuf::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn tmp_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".yaccgl-tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_paths() {
        assert_eq!(sanitize("Aniimo_Data\\Plugins\\x.dll"), Some(PathBuf::from("Aniimo_Data/Plugins/x.dll")));
        assert_eq!(sanitize("./a/b"), Some(PathBuf::from("a/b")));
        assert_eq!(sanitize("../etc/passwd"), None);
        assert_eq!(sanitize("a/../../b"), None);
        assert_eq!(sanitize("/abs"), None);
        assert_eq!(sanitize(""), None);
    }
}
