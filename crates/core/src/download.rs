//! Resumable, checksummed downloads with mirror fallback.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use md5::{Digest, Md5};

use crate::error::IoContext;
use crate::{Error, Result};

const MAX_ATTEMPTS: u32 = 6;
const BUF_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Downloading { done: u64, total: Option<u64> },
    Verifying { done: u64, total: u64 },
}

pub struct Request<'a> {
    /// Tried in order, rotating on failure.
    pub urls: &'a [String],
    pub dest: &'a Path,
    pub md5: &'a str,
    pub size: Option<u64>,
}

/// Download to `dest`, resuming from `dest.part` when present, and verify the MD5.
/// An existing `dest` with the right checksum is accepted without downloading.
pub fn fetch(
    agent: &ureq::Agent,
    req: &Request,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Transfer),
) -> Result<()> {
    if req.urls.is_empty() {
        return Err(Error::Http("no download URL available".into()));
    }
    if req.dest.exists() {
        if md5_file(req.dest, cancel, on)? == req.md5.to_ascii_lowercase() {
            return Ok(());
        }
        fs::remove_file(req.dest).io_ctx(|| format!("removing {}", req.dest.display()))?;
    }

    let part = part_path(req.dest);
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(Duration::from_secs(1 << attempt.min(4)));
        }
        let url = &req.urls[attempt as usize % req.urls.len()];
        match fetch_once(agent, url, &part, req.size, cancel, on) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e @ (Error::Cancelled | Error::Io { .. })) => return Err(e),
            Err(e) => last_err = Some(e),
        }
    }
    if let Some(e) = last_err {
        return Err(e);
    }

    let actual = md5_file(&part, cancel, on)?;
    if actual != req.md5.to_ascii_lowercase() {
        let _ = fs::remove_file(&part);
        return Err(Error::Checksum {
            path: req.dest.to_owned(),
            expected: req.md5.to_owned(),
            actual,
        });
    }
    fs::rename(&part, req.dest).io_ctx(|| format!("moving {} into place", part.display()))
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn fetch_once(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    size: Option<u64>,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Transfer),
) -> Result<()> {
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(false)
        .write(true)
        .truncate(false)
        .open(part)
        .io_ctx(|| format!("opening {}", part.display()))?;
    let mut start = file.metadata().map(|m| m.len()).unwrap_or(0);
    if size.is_some_and(|s| start > s) {
        start = 0;
    }
    if size == Some(start) && start > 0 {
        return Ok(());
    }

    let mut request = agent.get(url);
    if start > 0 {
        request = request.header("Range", format!("bytes={start}-"));
    }
    let mut response = match request.call() {
        // Range past the end: what we have is either complete or garbage.
        Err(ureq::Error::StatusCode(416)) if size.is_none_or(|s| s == start) => return Ok(()),
        Err(ureq::Error::StatusCode(416)) => {
            file.set_len(0).io_ctx(|| format!("truncating {}", part.display()))?;
            return Err(Error::Http("server rejected resume range; restarting".into()));
        }
        r => r?,
    };
    if response.status().as_u16() != 206 {
        // Server ignored the Range header and is sending the whole file.
        start = 0;
    }
    file.set_len(start).io_ctx(|| format!("truncating {}", part.display()))?;
    file.seek(SeekFrom::Start(start)).io_ctx(|| format!("seeking {}", part.display()))?;

    let total = size.or_else(|| response.body().content_length().map(|l| l + start));
    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut done = start;
    on(Transfer::Downloading { done, total });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::Http(format!("connection lost: {e}"))),
        };
        file.write_all(&buf[..n]).io_ctx(|| format!("writing {}", part.display()))?;
        done += n as u64;
        on(Transfer::Downloading { done, total });
    }
    file.flush().io_ctx(|| format!("writing {}", part.display()))?;
    if let Some(t) = total
        && done < t
    {
        return Err(Error::Http(format!("download ended early ({done} of {t} bytes)")));
    }
    Ok(())
}

/// Lowercase hex MD5 of a file, reporting progress.
pub fn md5_file(path: &Path, cancel: &AtomicBool, on: &mut dyn FnMut(Transfer)) -> Result<String> {
    let mut file = File::open(path).io_ctx(|| format!("opening {}", path.display()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = file.read(&mut buf).io_ctx(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        on(Transfer::Verifying { done, total });
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_of_known_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        fs::write(&p, b"hello world").unwrap();
        let sum = md5_file(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(sum, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn existing_file_with_matching_md5_skips_download() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("pkg.7z");
        fs::write(&dest, b"hello world").unwrap();
        // Unroutable URL: the test fails if a request is attempted.
        let urls = vec!["http://127.0.0.1:9/never".to_string()];
        let req = Request { urls: &urls, dest: &dest, md5: "5EB63BBBE01EEED093CB22BB8F5ACDC3", size: None };
        fetch(&crate::http::agent(), &req, &AtomicBool::new(false), &mut |_| {}).unwrap();
    }
}
