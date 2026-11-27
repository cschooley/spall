//! Task artifacts (#13 slice 3): a durable, listable, downloadable record a
//! task produces — a sandbox command's full output now, a packet capture
//! (slice 4) or a proof-of-concept (slice 7) later. Kept out of the task
//! log, which stays small and all-text: an event references an artifact by
//! id. Metadata lives in a per-engagement index at
//! `<engagements>/artifacts/<engagement>.json` (0600); the bytes are one
//! file each at `<engagements>/artifacts/data/<engagement>/<A-n>`, read on
//! demand so a large output never sits in memory or in the log.

use crate::engagement::{create_dir_private, write_atomic};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The most any one artifact may hold. Sandbox output is capped well under
/// this upstream (`sandbox::CAPTURE`); a packet capture can approach it.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// The most artifacts one engagement keeps.
pub const MAX_PER_ENGAGEMENT: usize = 5_000;
pub const MAX_LABEL: usize = 200;
pub const MAX_NOTE: usize = 500;
pub const MAX_CONTENT_TYPE: usize = 128;

/// What an artifact is, so the UI (and a later exporter) can tell them
/// apart without sniffing the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A sandbox command's full stdout and stderr.
    Output,
    /// A packet capture (slice 4).
    Pcap,
    /// A proof a PoC task built (slice 7).
    Poc,
    /// Anything else.
    Other,
}

/// An artifact's metadata. Never carries the bytes: [`Store::read`] fetches
/// those from disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// `A-<n>`, counting up per engagement.
    pub id: String,
    pub engagement: String,
    /// The task that produced it.
    pub task: String,
    pub kind: Kind,
    /// A short human label, filename-ish, shown in the UI.
    pub label: String,
    pub content_type: String,
    pub size: u64,
    /// SHA-256 of the bytes, hex: the tester can verify a download.
    pub sha256: String,
    pub created: String,
    /// One line on where it came from (e.g. the command's argv).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    Full,
    Save(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "no artifact {id}"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::Full => write!(
                f,
                "this engagement has {MAX_PER_ENGAGEMENT} artifacts already"
            ),
            Error::Save(m) => write!(f, "saving artifacts: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// What to store, the metadata half: [`Store::put`] takes it beside the
/// bytes.
pub struct New<'a> {
    pub engagement: &'a str,
    pub task: &'a str,
    pub kind: Kind,
    /// A short filename-ish label shown in the UI.
    pub label: &'a str,
    pub content_type: &'a str,
    /// One line on where it came from (e.g. the command's argv).
    pub note: &'a str,
}

#[derive(Serialize, Deserialize, Default)]
struct Book {
    next: u64,
    items: Vec<Artifact>,
}

/// Every engagement's artifact metadata, loaded at start; the bytes stay on
/// disk.
pub struct Store {
    dir: PathBuf,
    books: Mutex<HashMap<String, Book>>,
}

impl Store {
    /// Load `dir` (created if missing, mode 0700, with a `data/` subdir for
    /// the bytes). A metadata file that doesn't parse is an error: spall
    /// doesn't start with artifacts silently missing.
    pub fn open(dir: &Path) -> anyhow::Result<Store> {
        use anyhow::Context;
        create_dir_private(dir).with_context(|| format!("creating {}", dir.display()))?;
        create_dir_private(&dir.join("data"))?;
        let mut books = HashMap::new();
        for entry in fs_read_dir(dir)? {
            let p = entry.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue; // skips the `data/` directory and anything else
            }
            let Some(eng) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let book: Book = serde_json::from_slice(&std::fs::read(&p)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            books.insert(eng.to_string(), book);
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            books: Mutex::new(books),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Book>> {
        self.books.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn data_dir(&self, engagement: &str) -> PathBuf {
        self.dir.join("data").join(engagement)
    }

    /// Store `bytes` as a new artifact and return its metadata. The bytes
    /// are written first; only on success is the metadata index advanced, so
    /// a crash never leaves an index entry without its file.
    pub fn put(&self, meta: New<'_>, bytes: &[u8]) -> Result<Artifact, Error> {
        let New {
            engagement,
            task,
            kind,
            label,
            content_type,
            note,
        } = meta;
        if !valid(engagement) {
            return Err(Error::Invalid("not an engagement id".into()));
        }
        if !valid(task) {
            return Err(Error::Invalid("not a task id".into()));
        }
        if bytes.len() > MAX_BYTES {
            return Err(Error::Invalid(format!(
                "artifact too large ({} bytes; at most {MAX_BYTES})",
                bytes.len()
            )));
        }
        let label = {
            let l = clamp(label, MAX_LABEL);
            if l.is_empty() {
                "artifact".into()
            } else {
                l
            }
        };
        let content_type = {
            let t = clamp(content_type, MAX_CONTENT_TYPE);
            if t.is_empty() {
                "application/octet-stream".into()
            } else {
                t
            }
        };
        let note = clamp(note, MAX_NOTE);

        let mut books = self.lock();
        let book = books.entry(engagement.to_string()).or_default();
        if book.items.len() >= MAX_PER_ENGAGEMENT {
            return Err(Error::Full);
        }
        book.next = book.next.max(1);
        let id = format!("A-{}", book.next);

        let data_dir = self.data_dir(engagement);
        create_dir_private(&data_dir).map_err(|e| Error::Save(e.to_string()))?;
        let data_path = data_dir.join(&id);
        write_atomic(&data_path, bytes).map_err(|e| Error::Save(e.to_string()))?;

        let art = Artifact {
            id: id.clone(),
            engagement: engagement.into(),
            task: task.into(),
            kind,
            label,
            content_type,
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
            created: taphttp_types::Timestamp::now().to_string(),
            note,
        };
        book.items.push(art.clone());
        book.next += 1;
        if let Err(e) = save(&self.dir, engagement, book) {
            // Keep memory matching disk: undo the append and drop the file.
            book.items.pop();
            book.next -= 1;
            let _ = std::fs::remove_file(&data_path);
            return Err(Error::Save(e.to_string()));
        }
        Ok(art)
    }

    /// The engagement's artifacts, newest first.
    pub fn list(&self, engagement: &str) -> Vec<Artifact> {
        let mut v = self
            .lock()
            .get(engagement)
            .map(|b| b.items.clone())
            .unwrap_or_default();
        v.reverse();
        v
    }

    /// One task's artifacts, newest first.
    pub fn list_task(&self, engagement: &str, task: &str) -> Vec<Artifact> {
        self.list(engagement)
            .into_iter()
            .filter(|a| a.task == task)
            .collect()
    }

    /// One artifact's metadata.
    pub fn get(&self, engagement: &str, id: &str) -> Option<Artifact> {
        self.lock()
            .get(engagement)?
            .items
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    /// One artifact's bytes, read from disk; `None` if it's unknown or the
    /// file is gone.
    pub fn read(&self, engagement: &str, id: &str) -> Option<Vec<u8>> {
        self.get(engagement, id)?; // only read ids we wrote
        std::fs::read(self.data_dir(engagement).join(id)).ok()
    }
}

fn fs_read_dir(dir: &Path) -> anyhow::Result<Vec<std::fs::DirEntry>> {
    use anyhow::Context;
    std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("reading {}", dir.display()))
}

fn save(dir: &Path, engagement: &str, book: &Book) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(book)?;
    write_atomic(&dir.join(format!("{engagement}.json")), &bytes)
}

fn valid(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Trim, then cut to `max` bytes on a char boundary.
fn clamp(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.len() <= max {
        return t.to_string();
    }
    let mut end = max;
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].to_string()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_list_read_round_trip_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();

        let put = |task, kind, label, ct, note, bytes: &[u8]| {
            store.put(
                New {
                    engagement: "eng-1",
                    task,
                    kind,
                    label,
                    content_type: ct,
                    note,
                },
                bytes,
            )
        };
        let a = put(
            "T-1",
            Kind::Output,
            "nmap.txt",
            "text/plain",
            "nmap -sV",
            b"first",
        )
        .unwrap();
        let b = put("T-1", Kind::Output, "", "", "", b"second body").unwrap();
        // A different task, same engagement.
        let c = put("T-2", Kind::Poc, "poc.html", "text/html", "", b"<poc>").unwrap();

        assert_eq!(
            (a.id.as_str(), b.id.as_str(), c.id.as_str()),
            ("A-1", "A-2", "A-3")
        );
        // Empty label and content type get defaults.
        assert_eq!(
            (b.label.as_str(), b.content_type.as_str()),
            ("artifact", "application/octet-stream")
        );
        assert_eq!(a.size, 5);
        // sha256 of b"first", lowercase hex.
        assert_eq!(
            a.sha256,
            hex(&Sha256::digest(b"first")),
            "sha256 is over the bytes"
        );
        assert_eq!(a.sha256.len(), 64);
        assert!(a.sha256.chars().all(|ch| ch.is_ascii_hexdigit()));

        // Newest first; per-task filter.
        assert_eq!(
            store
                .list("eng-1")
                .iter()
                .map(|x| x.id.clone())
                .collect::<Vec<_>>(),
            ["A-3", "A-2", "A-1"]
        );
        assert_eq!(
            store
                .list_task("eng-1", "T-1")
                .iter()
                .map(|x| x.id.clone())
                .collect::<Vec<_>>(),
            ["A-2", "A-1"]
        );

        // Bytes read back exactly; unknown id is None.
        assert_eq!(store.read("eng-1", "A-1").unwrap(), b"first");
        assert_eq!(store.read("eng-1", "A-2").unwrap(), b"second body");
        assert!(store.read("eng-1", "A-9").is_none());
        assert!(store.read("eng-2", "A-1").is_none());

        // Reopen: metadata and the ids keep, the next id counts on.
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get("eng-1", "A-2").unwrap().task, "T-1");
        assert_eq!(store.read("eng-1", "A-3").unwrap(), b"<poc>");
        let d = store
            .put(
                New {
                    engagement: "eng-1",
                    task: "T-1",
                    kind: Kind::Output,
                    label: "x",
                    content_type: "text/plain",
                    note: "",
                },
                b"d",
            )
            .unwrap();
        assert_eq!(d.id, "A-4");
    }

    #[test]
    fn rejects_a_bad_id_or_an_oversized_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let put = |engagement, task, bytes: &[u8]| {
            store.put(
                New {
                    engagement,
                    task,
                    kind: Kind::Output,
                    label: "x",
                    content_type: "text/plain",
                    note: "",
                },
                bytes,
            )
        };
        assert!(matches!(put("eng/1", "T-1", b"x"), Err(Error::Invalid(_))));
        assert!(matches!(
            put("eng-1", "bad id", b"x"),
            Err(Error::Invalid(_))
        ));
        let big = vec![0u8; MAX_BYTES + 1];
        assert!(matches!(
            put("eng-1", "T-1", &big),
            Err(Error::Invalid(m)) if m.contains("too large")
        ));
        // Nothing was written.
        assert!(store.list("eng-1").is_empty());
    }
}
