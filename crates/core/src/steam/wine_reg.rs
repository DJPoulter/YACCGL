//! Minimal line-based editor for Wine's text registry files (`user.reg`).
//!
//! Sections look like `[Software\\Wine\\Explorer] 1790000000` (backslashes doubled),
//! followed by values such as `"Desktop"="Default"`. Lines we don't touch are kept
//! exactly as they are.

use std::time::{SystemTime, UNIX_EPOCH};

pub struct RegFile {
    lines: Vec<String>,
}

impl RegFile {
    pub fn parse(src: &str) -> RegFile {
        RegFile { lines: src.lines().map(str::to_owned).collect() }
    }

    pub fn serialize(&self) -> String {
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }

    /// String value `name` under the key `path` (written with single backslashes).
    pub fn get(&self, path: &str, name: &str) -> Option<String> {
        let (start, end) = self.section(path)?;
        self.lines[start + 1..end].iter().find_map(|l| {
            let rest = value_rest(l, name)?;
            unquote(rest)
        })
    }

    /// Set string value `name` under `path`, creating the key if needed.
    pub fn set(&mut self, path: &str, name: &str, value: &str) {
        let line = format!("{}={}", quote(name), quote(value));
        match self.section(path) {
            Some((start, end)) => {
                if let Some(i) = (start + 1..end).find(|&i| value_rest(&self.lines[i], name).is_some()) {
                    self.lines[i] = line;
                } else {
                    // After the section's last non-blank line.
                    let at = (start + 1..end).rev().find(|&i| !self.lines[i].trim().is_empty()).map_or(start + 1, |i| i + 1);
                    self.lines.insert(at, line);
                }
            }
            None => {
                if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    self.lines.push(String::new());
                }
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                self.lines.push(format!("[{}] {now}", path.replace('\\', "\\\\")));
                self.lines.push(line);
            }
        }
    }

    /// Remove value `name` under `path`. Returns whether it existed.
    pub fn remove(&mut self, path: &str, name: &str) -> bool {
        let Some((start, end)) = self.section(path) else { return false };
        match (start + 1..end).find(|&i| value_rest(&self.lines[i], name).is_some()) {
            Some(i) => {
                self.lines.remove(i);
                true
            }
            None => false,
        }
    }

    /// Line range of a key: (header index, index of the next header or end).
    fn section(&self, path: &str) -> Option<(usize, usize)> {
        let header = format!("[{}]", path.replace('\\', "\\\\"));
        let start = self.lines.iter().position(|l| {
            l.get(..header.len()).is_some_and(|h| h.eq_ignore_ascii_case(&header))
        })?;
        let end = (start + 1..self.lines.len()).find(|&i| self.lines[i].starts_with('[')).unwrap_or(self.lines.len());
        Some((start, end))
    }
}

/// If `line` is `"name"=...`, return the part after `=`.
fn value_rest<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let q = quote(name);
    let head = line.get(..q.len())?;
    head.eq_ignore_ascii_case(&q).then(|| line[q.len()..].strip_prefix('='))?
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn unquote(s: &str) -> Option<String> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next()? } else { c });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n[Control Panel\\\\Desktop] 1700000000\n#time=1da0000000000\n\"FontSmoothing\"=\"2\"\n\n[Software\\\\Wine\\\\Explorer] 1700000000\n#time=1da0000000000\n\"Desktop\"=\"Other\"\n\n[Software\\\\Wine\\\\Explorer\\\\Desktops] 1700000000\n\"Other\"=\"800x600\"\n";

    #[test]
    fn untouched_file_round_trips() {
        assert_eq!(RegFile::parse(SAMPLE).serialize(), SAMPLE);
    }

    #[test]
    fn reads_and_updates_existing_values() {
        let mut r = RegFile::parse(SAMPLE);
        assert_eq!(r.get("Software\\Wine\\Explorer", "Desktop").as_deref(), Some("Other"));
        // Section names match exactly, not by prefix.
        assert_eq!(r.get("Software\\Wine\\Explorer\\Desktops", "Desktop"), None);
        r.set("Software\\Wine\\Explorer", "Desktop", "Default");
        r.set("Software\\Wine\\Explorer\\Desktops", "Default", "1280x800");
        let out = r.serialize();
        assert!(out.contains("[Software\\\\Wine\\\\Explorer] 1700000000\n#time=1da0000000000\n\"Desktop\"=\"Default\"\n"));
        assert!(out.contains("\"Other\"=\"800x600\"\n\"Default\"=\"1280x800\"\n"));
        assert!(out.contains("\"FontSmoothing\"=\"2\""));
    }

    #[test]
    fn creates_missing_keys_and_removes_values() {
        let mut r = RegFile::parse("WINE REGISTRY Version 2\n\n[Control Panel\\\\Desktop] 1\n\"A\"=\"b\"\n");
        r.set("Software\\Wine\\Explorer", "Desktop", "Default");
        let reparsed = RegFile::parse(&r.serialize());
        assert_eq!(reparsed.get("Software\\Wine\\Explorer", "Desktop").as_deref(), Some("Default"));
        assert!(r.serialize().contains("\n\n[Software\\\\Wine\\\\Explorer] "));

        assert!(r.remove("software\\wine\\explorer", "desktop"));
        assert!(!r.remove("Software\\Wine\\Explorer", "Desktop"));
        assert_eq!(r.get("Software\\Wine\\Explorer", "Desktop"), None);
    }

    #[test]
    fn escapes_values() {
        let mut r = RegFile::parse("");
        r.set("K", "path", "C:\\x \"y\"");
        assert!(r.serialize().contains(r#""path"="C:\\x \"y\"""#));
        assert_eq!(r.get("K", "path").as_deref(), Some("C:\\x \"y\""));
    }
}
