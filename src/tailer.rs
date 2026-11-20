//! Follows a growing JSON-lines file, emitting complete lines.
//!
//! Polling, not inotify: the log is written by another container on a
//! shared volume, where filesystem events are unreliable. Handles the
//! file not existing yet, truncation, replacement (new inode), partial
//! trailing lines, and pathologically long lines.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Lines longer than this are dropped instead of buffered without limit.
pub const DEFAULT_MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

pub struct LineTailer {
    path: PathBuf,
    offset: u64,
    file_id: Option<u64>,
    buf: Vec<u8>,
    /// True while discarding the rest of an oversized line.
    skipping: bool,
    max_line_bytes: usize,
    read_budget: u64,
    waiting_logged: bool,
}

impl LineTailer {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_max_line_bytes(path, DEFAULT_MAX_LINE_BYTES)
    }

    pub fn with_max_line_bytes(path: impl Into<PathBuf>, max_line_bytes: usize) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            file_id: None,
            buf: Vec::new(),
            skipping: false,
            max_line_bytes,
            read_budget: READ_BUDGET_BYTES,
            waiting_logged: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.buf.clear();
        self.skipping = false;
    }

    /// Read whatever has been appended since the last poll and return the
    /// complete lines (without the trailing newline / CRLF). Blank lines
    /// and lines that are not valid UTF-8 are skipped.
    ///
    /// Reads at most [`READ_BUDGET_BYTES`] per call so a large existing log
    /// is consumed incrementally rather than loaded into memory at once.
    ///
    /// Limitation: a truncate-and-regrow to at least the previous size
    /// between two polls (same inode) is indistinguishable from an append.
    pub fn poll(&mut self) -> io::Result<Vec<String>> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !self.waiting_logged {
                    tracing::info!("waiting for {} to appear", self.path.display());
                    self.waiting_logged = true;
                }
                self.reset();
                self.file_id = None;
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };
        if self.waiting_logged {
            tracing::info!("{} appeared; reading", self.path.display());
            self.waiting_logged = false;
        }

        let meta = file.metadata()?;
        let id = file_id(&meta);
        if self.file_id.is_some() && id != self.file_id {
            tracing::info!("{} was replaced; reading from start", self.path.display());
            self.reset();
        }
        self.file_id = id;

        let len = meta.len();
        if len < self.offset {
            tracing::info!("{} was truncated; reading from start", self.path.display());
            self.reset();
        }
        if len == self.offset {
            return Ok(Vec::new());
        }

        file.seek(SeekFrom::Start(self.offset))?;
        let want = (len - self.offset).min(self.read_budget);
        let mut chunk = Vec::new();
        file.take(want).read_to_end(&mut chunk)?;
        self.offset += chunk.len() as u64;
        Ok(self.consume(&chunk))
    }

    fn consume(&mut self, mut data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        while !data.is_empty() {
            match data.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    let head = &data[..i];
                    data = &data[i + 1..];
                    if self.skipping {
                        self.skipping = false;
                        continue;
                    }
                    if self.buf.len() + head.len() > self.max_line_bytes {
                        self.warn_oversized();
                        self.buf.clear();
                        continue;
                    }
                    self.buf.extend_from_slice(head);
                    if let Some(line) = finish_line(std::mem::take(&mut self.buf)) {
                        out.push(line);
                    }
                }
                None => {
                    if !self.skipping {
                        if self.buf.len() + data.len() > self.max_line_bytes {
                            self.warn_oversized();
                            self.buf.clear();
                            self.skipping = true;
                        } else {
                            self.buf.extend_from_slice(data);
                        }
                    }
                    break;
                }
            }
        }
        out
    }

    fn warn_oversized(&self) {
        tracing::warn!(
            "skipping line longer than {} bytes in {}",
            self.max_line_bytes,
            self.path.display()
        );
    }
}

/// Upper bound on bytes read from the file in one [`LineTailer::poll`].
pub const READ_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

fn finish_line(mut bytes: Vec<u8>) -> Option<String> {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.is_empty() {
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(s) => Some(s),
        Err(_) => {
            tracing::warn!("skipping line that is not valid UTF-8");
            None
        }
    }
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ino())
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn append(path: &Path, s: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(s.as_bytes()).unwrap();
    }

    #[test]
    fn missing_file_yields_nothing_then_picks_up_once_created() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("traffic.jsonl");
        let mut t = LineTailer::new(&path);
        assert!(t.poll().unwrap().is_empty());
        assert!(t.poll().unwrap().is_empty());
        append(&path, "one\n");
        assert_eq!(t.poll().unwrap(), vec!["one"]);
    }

    #[test]
    fn reads_existing_content_then_only_new_appends() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "a\nb\n");
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap(), vec!["a", "b"]);
        assert!(t.poll().unwrap().is_empty());
        append(&path, "c\n");
        assert_eq!(t.poll().unwrap(), vec!["c"]);
    }

    #[test]
    fn partial_line_is_held_until_newline_arrives() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "{\"id\":");
        let mut t = LineTailer::new(&path);
        assert!(t.poll().unwrap().is_empty());
        append(&path, "\"x\"}");
        assert!(t.poll().unwrap().is_empty());
        append(&path, "\nnext\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"id\":\"x\"}", "next"]);
    }

    #[test]
    fn truncation_restarts_from_beginning() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "old-1\nold-2\nold-3\n");
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap().len(), 3);
        // Truncate in place (same inode) and write shorter content.
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        append(&path, "new\n");
        assert_eq!(t.poll().unwrap(), vec!["new"]);
    }

    #[test]
    fn truncation_discards_buffered_partial_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "complete\npartial-from-old-file");
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap(), vec!["complete"]);
        std::fs::write(&path, "").unwrap();
        append(&path, "fresh\n");
        assert_eq!(t.poll().unwrap(), vec!["fresh"]);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_file_is_read_from_start() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "aaaaaaaa\nbbbbbbbb\n");
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap().len(), 2);
        // Replace via rename: new inode, and longer than the old offset.
        let tmp = dir.path().join("new.jsonl");
        append(&tmp, "replacement-line-1\nreplacement-line-2\n");
        std::fs::rename(&tmp, &path).unwrap();
        assert_eq!(
            t.poll().unwrap(),
            vec!["replacement-line-1", "replacement-line-2"]
        );
    }

    #[test]
    fn deleted_then_recreated_file_is_read_from_start() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "first\n");
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap(), vec!["first"]);
        std::fs::remove_file(&path).unwrap();
        assert!(t.poll().unwrap().is_empty());
        append(&path, "second\n");
        assert_eq!(t.poll().unwrap(), vec!["second"]);
    }

    #[test]
    fn crlf_and_blank_lines_are_handled() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "a\r\n\n\r\nb\n");
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn invalid_utf8_line_is_skipped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"ok-1\n\xff\xfe\nok-2\n").unwrap();
        let mut t = LineTailer::new(&path);
        assert_eq!(t.poll().unwrap(), vec!["ok-1", "ok-2"]);
    }

    #[test]
    fn oversized_line_is_dropped_and_following_lines_survive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut t = LineTailer::with_max_line_bytes(&path, 8);
        append(&path, "short\n0123456789");
        assert_eq!(t.poll().unwrap(), vec!["short"]);
        append(&path, "abcdef");
        assert!(t.poll().unwrap().is_empty());
        append(&path, "-end-of-long\nafter\n");
        assert_eq!(t.poll().unwrap(), vec!["after"]);
    }

    #[test]
    fn large_backlog_is_read_across_several_polls() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "line-1\nline-2\nline-3\n");
        let mut t = LineTailer::new(&path);
        t.read_budget = 10;
        let mut all = Vec::new();
        for _ in 0..10 {
            all.extend(t.poll().unwrap());
        }
        assert_eq!(all, vec!["line-1", "line-2", "line-3"]);
    }

    #[test]
    fn oversized_line_in_one_read_is_dropped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "0123456789abcdef\nok\n");
        let mut t = LineTailer::with_max_line_bytes(&path, 8);
        assert_eq!(t.poll().unwrap(), vec!["ok"]);
    }
}
