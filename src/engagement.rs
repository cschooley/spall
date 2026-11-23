//! The engagement (#35): spall's first object of its own, persisted under
//! the data volume. It holds who the test is for, when it runs, and its
//! targets: the scope the gate (#22, #34) enforces. One engagement is
//! active at a time; switching is allowed and audited.
//!
//! One JSON file per engagement in `--engagements` (default
//! `/data/engagements`), written atomically (temp file, fsync, rename) with
//! mode 0600, plus an `active` file naming the active one. A file that
//! doesn't parse, or has a newer schema, stops spall instead of silently
//! starting with a different scope.
//!
//! Its rules of engagement (#36, `roe`) travel with it.

use crate::gate::Scope;
use crate::openapi::ApiSpec;
use crate::roe::{Blackout, Period, Rules, Window};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Engagement {
    pub schema_version: u32,
    /// Lowercase letters, digits and `-`; also the file name.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub client: String,
    /// Dates as `YYYY-MM-DD`.
    #[serde(default)]
    pub starts: Option<String>,
    #[serde(default)]
    pub ends: Option<String>,
    /// When the authorization to test runs out (may differ from `ends`).
    #[serde(default)]
    pub expires: Option<String>,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub notes: String,
    /// Scope entries as `Scope::entries` writes them (`!entry` excluded),
    /// never `*`: allowing every host is for the running process only.
    #[serde(default)]
    pub scope: Vec<String>,
    /// A note per scope entry ("staging only", "owned by the client's CDN").
    #[serde(default)]
    pub target_notes: BTreeMap<String, String>,
    /// Rules of engagement (#36). Files from before #36 get the defaults.
    #[serde(default)]
    pub rules: Rules,
    /// Imported API descriptions (#40): servers and paths, for Surface.
    #[serde(default)]
    pub api_specs: Vec<ApiSpec>,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
}

/// A change to the active engagement's details (`PATCH /api/engagement`,
/// `engagement set`). Absent fields are left alone; an empty string clears
/// a date. Scope is changed through `/api/scope`, never here.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub name: Option<String>,
    pub client: Option<String>,
    pub starts: Option<String>,
    pub ends: Option<String>,
    pub expires: Option<String>,
    pub contacts: Option<Vec<Contact>>,
    pub notes: Option<String>,
    pub target_notes: Option<BTreeMap<String, String>>,
    pub rules: Option<RulesPatch>,
    pub api_specs: Option<Vec<ApiSpec>>,
}

/// A change to the rules of engagement; absent fields are left alone.
/// `rate_limit: 0` switches the ceiling off.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesPatch {
    pub methods: Option<Vec<String>>,
    pub safe_methods: Option<Vec<String>>,
    pub rate_limit: Option<f64>,
    pub time_zone: Option<String>,
    pub windows: Option<Vec<Window>>,
    pub blackouts: Option<Vec<Blackout>>,
    pub agents_allowed: Option<bool>,
    pub destructive_allowed: Option<bool>,
}

/// A new engagement (`POST /api/engagements`, `engagement new`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct New {
    pub name: String,
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub client: String,
    pub active: bool,
}

/// Fields that changed, as `{field: {before, after}}`, for the audit log.
pub type Changes = BTreeMap<String, serde_json::Value>;

fn text_ok(s: &str, max: usize, multiline: bool) -> bool {
    s.chars().count() <= max
        && s.chars()
            .all(|c| !c.is_control() || (multiline && (c == '\n' || c == '\t')))
}

fn field(name: &str, s: &str, max: usize, multiline: bool) -> Result<(), String> {
    if text_ok(s, max, multiline) {
        Ok(())
    } else {
        Err(format!(
            "{name}: at most {max} characters{}",
            if multiline { "" } else { ", one line" }
        ))
    }
}

fn parse_date(name: &str, s: &str) -> Result<jiff::civil::Date, String> {
    let ok = s.len() == 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-';
    let date = ok.then(|| s.parse::<jiff::civil::Date>().ok()).flatten();
    date.ok_or_else(|| format!("{name}: want a date as YYYY-MM-DD, got {s:?}"))
}

impl Engagement {
    /// Check every field; the store refuses to save or load an invalid one.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!("id {:?}: lowercase letters, digits and -", self.id));
        }
        if self.name.trim().is_empty() {
            return Err("name: required".into());
        }
        field("name", &self.name, 120, false)?;
        field("client", &self.client, 120, false)?;
        field("notes", &self.notes, 10_000, true)?;
        let dates: Vec<(&str, &Option<String>)> = vec![
            ("starts", &self.starts),
            ("ends", &self.ends),
            ("expires", &self.expires),
        ];
        for (name, d) in &dates {
            if let Some(d) = d {
                parse_date(name, d)?;
            }
        }
        if let (Some(s), Some(e)) = (&self.starts, &self.ends) {
            if parse_date("ends", e)? < parse_date("starts", s)? {
                return Err("ends: before starts".into());
            }
        }
        if self.contacts.len() > 20 {
            return Err("contacts: at most 20".into());
        }
        for c in &self.contacts {
            for (n, v) in [
                ("name", &c.name),
                ("role", &c.role),
                ("email", &c.email),
                ("phone", &c.phone),
            ] {
                field(&format!("contact {n}"), v, 200, false)?;
            }
        }
        let scope = self.parsed_scope()?;
        if self.scope.iter().any(|e| e == "*") {
            return Err("scope: `*` is for the running process only (`scope all on`)".into());
        }
        if scope.entries() != self.scope {
            return Err("scope: entries aren't in normal form".into());
        }
        self.rules
            .validate()
            .map_err(|e| format!("rules of engagement: {e}"))?;
        if self.target_notes.len() > 500 {
            return Err("target notes: at most 500".into());
        }
        for (k, v) in &self.target_notes {
            field("target note entry", k, 260, false)?;
            field(&format!("note for {k}"), v, 1000, true)?;
        }
        Ok(())
    }

    /// The engagement's scope (never allowing every host).
    pub fn parsed_scope(&self) -> Result<Scope, String> {
        Scope::parse(&self.scope.join(","))
    }

    fn summary(&self, active: bool) -> Summary {
        Summary {
            id: self.id.clone(),
            name: self.name.clone(),
            client: self.client.clone(),
            active,
        }
    }

    /// Apply a patch; returns what changed. Doesn't validate.
    fn apply(&mut self, p: Patch) -> Changes {
        let mut changes = Changes::new();
        let mut note = |name: &str, before: serde_json::Value, after: serde_json::Value| {
            if before != after {
                changes.insert(
                    name.into(),
                    serde_json::json!({ "before": before, "after": after }),
                );
            }
        };
        let date = |v: String| {
            let v = v.trim().to_string();
            (!v.is_empty()).then_some(v)
        };
        macro_rules! set {
            ($f:ident, $v:expr) => {{
                let after = $v;
                note(
                    stringify!($f),
                    serde_json::json!(self.$f),
                    serde_json::json!(after),
                );
                self.$f = after;
            }};
        }
        if let Some(v) = p.name {
            set!(name, v.trim().to_string());
        }
        if let Some(v) = p.client {
            set!(client, v.trim().to_string());
        }
        if let Some(v) = p.starts {
            set!(starts, date(v));
        }
        if let Some(v) = p.ends {
            set!(ends, date(v));
        }
        if let Some(v) = p.expires {
            set!(expires, date(v));
        }
        if let Some(v) = p.contacts {
            set!(contacts, v);
        }
        if let Some(v) = p.notes {
            set!(notes, v);
        }
        if let Some(v) = p.target_notes {
            set!(target_notes, v);
        }
        if let Some(v) = p.api_specs {
            set!(api_specs, v);
        }
        if let Some(r) = p.rules {
            let upper = |v: Vec<String>| -> Vec<String> {
                v.iter()
                    .map(|m| m.trim().to_ascii_uppercase())
                    .filter(|m| !m.is_empty())
                    .collect()
            };
            macro_rules! rule {
                ($f:ident, $v:expr) => {{
                    let after = $v;
                    note(
                        concat!("rules.", stringify!($f)),
                        serde_json::json!(self.rules.$f),
                        serde_json::json!(after),
                    );
                    self.rules.$f = after;
                }};
            }
            if let Some(v) = r.methods {
                rule!(methods, upper(v));
            }
            if let Some(v) = r.safe_methods {
                rule!(safe_methods, upper(v));
            }
            if let Some(v) = r.rate_limit {
                rule!(rate_limit, (v != 0.0).then_some(v));
            }
            if let Some(v) = r.time_zone {
                rule!(time_zone, v.trim().to_string());
            }
            if let Some(v) = r.windows {
                rule!(windows, v);
            }
            if let Some(v) = r.blackouts {
                rule!(blackouts, v);
            }
            if let Some(v) = r.agents_allowed {
                rule!(agents_allowed, v);
            }
            if let Some(v) = r.destructive_allowed {
                rule!(destructive_allowed, v);
            }
        }
        changes
    }

    /// The dates the rules check against.
    pub fn period(&self) -> Period {
        Period {
            starts: self.starts.clone(),
            ends: self.ends.clone(),
            expires: self.expires.clone(),
        }
    }
}

fn valid_id(id: &str) -> bool {
    (1..=48).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

/// A file-safe id from a name, with a random suffix: `acme-q4-3f9a`.
fn new_id(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 32 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let mut rnd = [0u8; 2];
    // Uniqueness also comes from the existence check; zeros are fine on failure.
    let _ = getrandom::fill(&mut rnd);
    let hex = format!("{:02x}{:02x}", rnd[0], rnd[1]);
    if slug.is_empty() {
        format!("engagement-{hex}")
    } else {
        format!("{slug}-{hex}")
    }
}

#[derive(Debug)]
pub enum Error {
    /// Bad input: a message safe to show.
    Invalid(String),
    NotFound(String),
    /// Couldn't write: nothing changed.
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Invalid(m) | Error::NotFound(m) | Error::Io(m) => f.write_str(m),
        }
    }
}

struct State {
    active: Engagement,
    scope: Scope,
}

pub struct Store {
    dir: PathBuf,
    state: Mutex<State>,
    /// Rules a new engagement starts with (from --safe-methods).
    defaults: Rules,
}

/// What opening the store did, for startup logging.
#[derive(Debug, Default)]
pub struct Opened {
    /// A default engagement was created from --scope.
    pub bootstrapped: bool,
    /// --scope was given but the active engagement's scope differs.
    pub config_ignored: bool,
    /// Engagement files skipped because they don't load (not the active one).
    pub skipped: Vec<String>,
}

fn now() -> String {
    taphttp_types::Timestamp::now().to_string()
}

impl Store {
    /// Open (creating the directory, mode 0700) and load the active
    /// engagement. With none on disk, create one from `bootstrap` (the
    /// --scope config, `*` dropped: that stays a process flag).
    pub fn open(
        dir: &Path,
        bootstrap: &Scope,
        defaults: &Rules,
    ) -> anyhow::Result<(Store, Opened)> {
        use anyhow::Context;
        create_dir_private(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut opened = Opened::default();
        let active_file = dir.join("active");
        let active = match fs::read_to_string(&active_file) {
            Ok(id) => {
                let id = id.trim();
                anyhow::ensure!(
                    valid_id(id),
                    "{}: not an engagement id",
                    active_file.display()
                );
                let e = load(&dir.join(format!("{id}.json")))?;
                let config = bootstrap.with_all(false);
                if !config.is_empty() && e.parsed_scope().ok().as_ref() != Some(&config) {
                    opened.config_ignored = true;
                }
                e
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let ts = now();
                let e = Engagement {
                    schema_version: SCHEMA_VERSION,
                    id: new_id("engagement"),
                    name: "Engagement".into(),
                    client: String::new(),
                    starts: None,
                    ends: None,
                    expires: None,
                    contacts: Vec::new(),
                    notes: String::new(),
                    scope: bootstrap.with_all(false).entries(),
                    target_notes: BTreeMap::new(),
                    rules: defaults.clone(),
                    api_specs: Vec::new(),
                    created: ts.clone(),
                    updated: ts,
                };
                save(dir, &e)?;
                write_atomic(&active_file, e.id.as_bytes())?;
                opened.bootstrapped = true;
                e
            }
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", active_file.display()))
            }
        };
        let store = Store {
            dir: dir.to_path_buf(),
            state: Mutex::new(State {
                scope: active.parsed_scope().map_err(anyhow::Error::msg)?,
                active,
            }),
            defaults: defaults.clone(),
        };
        let (_, skipped) = store.load_all();
        opened.skipped = skipped;
        Ok((store, opened))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn active(&self) -> Engagement {
        self.lock().active.clone()
    }

    /// The active engagement's scope (never allowing every host).
    pub fn scope(&self) -> Scope {
        self.lock().scope.clone()
    }

    /// Change the active engagement's scope with `f`, and save it. Returns
    /// (before, after, whether anything changed). On error nothing changes.
    pub fn change_scope<T>(
        &self,
        f: impl FnOnce(&mut Scope) -> Result<T, String>,
    ) -> Result<(Scope, Scope, T), Error> {
        let mut st = self.lock();
        let before = st.scope.clone();
        let mut after = before.clone();
        let out = f(&mut after).map_err(Error::Invalid)?;
        if after != before {
            let mut e = st.active.clone();
            e.scope = after.entries();
            e.updated = now();
            save(&self.dir, &e)
                .map_err(|err| Error::Io(format!("saving the engagement: {err:#}")))?;
            st.active = e;
            st.scope = after.clone();
        }
        Ok((before, after, out))
    }

    /// Change the active engagement's details. Returns the result and what
    /// changed. On error nothing changes.
    pub fn update(&self, patch: Patch) -> Result<(Engagement, Changes), Error> {
        let mut st = self.lock();
        let mut e = st.active.clone();
        let changes = e.apply(patch);
        if changes.is_empty() {
            return Ok((e, changes));
        }
        e.validate().map_err(Error::Invalid)?;
        e.updated = now();
        save(&self.dir, &e).map_err(|err| Error::Io(format!("saving the engagement: {err:#}")))?;
        st.active = e.clone();
        Ok((e, changes))
    }

    /// Every engagement on disk, active first then by name.
    pub fn list(&self) -> Vec<Summary> {
        let active = self.lock().active.id.clone();
        let (mut all, _) = self.load_all();
        all.sort_by(|a, b| {
            (a.id != active, a.name.to_lowercase()).cmp(&(b.id != active, b.name.to_lowercase()))
        });
        all.iter().map(|e| e.summary(e.id == active)).collect()
    }

    fn load_all(&self) -> (Vec<Engagement>, Vec<String>) {
        let mut ok = Vec::new();
        let mut skipped = Vec::new();
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return (ok, skipped);
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match load(&path) {
                Ok(e) => ok.push(e),
                Err(err) => skipped.push(format!("{}: {err:#}", path.display())),
            }
        }
        (ok, skipped)
    }

    /// Create an engagement and make it active.
    pub fn create(&self, new: New) -> Result<Engagement, Error> {
        let mut scope = Scope::default();
        for entry in &new.scope {
            let e = entry.trim();
            let r = match e.strip_prefix('!') {
                Some(ex) => scope.exclude(ex),
                None => scope.add(e),
            };
            r.map_err(Error::Invalid)?;
        }
        let ts = now();
        let mut e = Engagement {
            schema_version: SCHEMA_VERSION,
            id: new_id(&new.name),
            name: new.name.trim().to_string(),
            client: new.client.trim().to_string(),
            starts: None,
            ends: None,
            expires: None,
            contacts: Vec::new(),
            notes: String::new(),
            scope: scope.entries(),
            target_notes: BTreeMap::new(),
            rules: self.defaults.clone(),
            api_specs: Vec::new(),
            created: ts.clone(),
            updated: ts,
        };
        while self.dir.join(format!("{}.json", e.id)).exists() {
            e.id = new_id(&e.name);
        }
        e.validate().map_err(Error::Invalid)?;
        let mut st = self.lock();
        save(&self.dir, &e).map_err(|err| Error::Io(format!("saving the engagement: {err:#}")))?;
        self.set_active(&mut st, e.clone())?;
        Ok(e)
    }

    /// Make an engagement active, by id or exact name (case-insensitive).
    pub fn activate(&self, which: &str) -> Result<(Engagement, Engagement), Error> {
        let which = which.trim();
        let (all, _) = self.load_all();
        let matches: Vec<&Engagement> = all
            .iter()
            .filter(|e| e.id == which || e.name.eq_ignore_ascii_case(which))
            .collect();
        let target = match matches.as_slice() {
            [one] => (*one).clone(),
            [] => return Err(Error::NotFound(format!("no engagement {which:?}"))),
            _ => {
                return Err(Error::Invalid(format!(
                    "{} engagements are named {which:?}; use an id",
                    matches.len()
                )))
            }
        };
        let mut st = self.lock();
        let before = st.active.clone();
        self.set_active(&mut st, target.clone())?;
        Ok((before, target))
    }

    fn set_active(&self, st: &mut State, e: Engagement) -> Result<(), Error> {
        let scope = e.parsed_scope().map_err(Error::Invalid)?;
        write_atomic(&self.dir.join("active"), e.id.as_bytes())
            .map_err(|err| Error::Io(format!("switching engagement: {err:#}")))?;
        st.active = e;
        st.scope = scope;
        Ok(())
    }
}

fn load(path: &Path) -> anyhow::Result<Engagement> {
    use anyhow::Context;
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{}: not JSON", path.display()))?;
    let version = v.get("schema_version").and_then(|s| s.as_u64());
    anyhow::ensure!(
        version == Some(u64::from(SCHEMA_VERSION)),
        "{}: schema_version {version:?}; this spall reads {SCHEMA_VERSION} (upgrade spall, or move the file aside)",
        path.display()
    );
    let e: Engagement = serde_json::from_value(v)
        .with_context(|| format!("{}: not an engagement", path.display()))?;
    e.validate()
        .map_err(|m| anyhow::anyhow!("{}: {m}", path.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str());
    anyhow::ensure!(
        stem == Some(e.id.as_str()),
        "{}: id {:?} doesn't match the file name",
        path.display(),
        e.id
    );
    Ok(e)
}

fn save(dir: &Path, e: &Engagement) -> anyhow::Result<()> {
    let mut body = serde_json::to_vec_pretty(e)?;
    body.push(b'\n');
    write_atomic(&dir.join(format!("{}.json", e.id)), &body)
}

pub(crate) fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Write to a temp file beside `path`, fsync, rename over it, fsync the
/// directory: a crash leaves the old file or the new one, never half.
pub(crate) fn write_atomic(path: &Path, body: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;
    let dir = path.parent().context("no parent directory")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("bad file name")?;
    let mut rnd = [0u8; 4];
    getrandom::fill(&mut rnd).map_err(|e| anyhow::anyhow!("random: {e}"))?;
    let tmp = dir.join(format!(".{name}.{:08x}.tmp", u32::from_le_bytes(rnd)));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(body)?;
        f.sync_all()?;
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(dir: &Path, scope: &str) -> (Store, Opened) {
        Store::open(dir, &Scope::parse(scope).unwrap(), &Rules::default()).unwrap()
    }

    #[test]
    fn bootstraps_from_config_once_then_keeps_its_own_scope() {
        let dir = tempfile::TempDir::new().unwrap();
        let (s, o) = open(dir.path(), "a.test, *.b.test, !x.b.test, *");
        assert!(o.bootstrapped);
        assert_eq!(
            s.active().scope,
            ["a.test", "*.b.test", "!x.b.test"],
            "* is not saved"
        );
        assert!(!s.scope().allows_all());
        s.change_scope(|sc| sc.add("c.test")).unwrap();
        drop(s);
        let (s, o) = open(dir.path(), "a.test");
        assert!(!o.bootstrapped);
        assert!(o.config_ignored, "config disagrees with the saved scope");
        assert_eq!(
            s.active().scope,
            ["a.test", "*.b.test", "c.test", "!x.b.test"]
        );
        let (_, o) = open(dir.path(), "");
        assert!(
            !o.config_ignored,
            "no config scope: nothing to disagree with"
        );
    }

    #[test]
    fn files_are_private_and_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("e");
        let (s, _) = open(&path, "a.test");
        let id = s.active().id;
        let file = path.join(format!("{id}.json"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(load(&file).unwrap(), s.active());
        let leftovers: Vec<_> = fs::read_dir(&path)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_corrupt_or_newer_active_file_refuses_to_load() {
        for bad in [
            "{not json".to_string(),
            r#"{"schema_version": 2, "id": "x", "name": "x", "created": "", "updated": ""}"#.into(),
            r#"{"schema_version": 1, "id": "x", "name": "x", "scope": ["http://nope"], "created": "", "updated": ""}"#.into(),
            r#"{"schema_version": 1, "id": "other", "name": "x", "created": "", "updated": ""}"#.into(),
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            fs::write(dir.path().join("active"), "x").unwrap();
            fs::write(dir.path().join("x.json"), &bad).unwrap();
            let err = Store::open(dir.path(), &Scope::parse("a.test").unwrap(), &Rules::default()).err();
            assert!(err.is_some(), "{bad}");
            assert_eq!(fs::read_to_string(dir.path().join("x.json")).unwrap(), bad, "left alone");
        }
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("active"), "../etc/passwd").unwrap();
        assert!(Store::open(dir.path(), &Scope::default(), &Rules::default()).is_err());
    }

    #[test]
    fn create_list_and_switch() {
        let dir = tempfile::TempDir::new().unwrap();
        let (s, _) = open(dir.path(), "a.test");
        let first = s.active().id;
        let e = s
            .create(New {
                name: "ACME Q4 / web".into(),
                client: "ACME".into(),
                scope: vec!["*.acme.test".into(), "!pay.acme.test".into()],
            })
            .unwrap();
        assert!(e.id.starts_with("acme-q4-web-"), "{}", e.id);
        assert_eq!(s.active().id, e.id, "new engagements become active");
        assert!(!s.scope().contains("pay.acme.test") && s.scope().contains("www.acme.test"));
        let names: Vec<_> = s.list().into_iter().map(|x| (x.name, x.active)).collect();
        assert_eq!(
            names,
            [
                ("ACME Q4 / web".to_string(), true),
                ("Engagement".into(), false)
            ]
        );
        let (before, after) = s.activate("engagement").unwrap();
        assert_eq!((before.id, after.id.clone()), (e.id.clone(), first.clone()));
        assert!(s.scope().contains("a.test"));
        assert!(matches!(s.activate("nope"), Err(Error::NotFound(_))));
        drop(s);
        let (s, _) = open(dir.path(), "");
        assert_eq!(s.active().id, first, "the active one survives a restart");
        assert!(s
            .create(New {
                name: " ".into(),
                ..New::default()
            })
            .is_err());
        assert!(s
            .create(New {
                name: "x".into(),
                scope: vec!["*".into()],
                ..New::default()
            })
            .is_err());
    }

    #[test]
    fn updates_are_validated_and_report_what_changed() {
        let dir = tempfile::TempDir::new().unwrap();
        let (s, _) = open(dir.path(), "a.test");
        let (e, changes) = s
            .update(Patch {
                name: Some(" ACME ".into()),
                starts: Some("2026-11-23".into()),
                ends: Some("2026-12-04".into()),
                ..Patch::default()
            })
            .unwrap();
        assert_eq!(e.name, "ACME");
        assert_eq!(
            changes["name"],
            serde_json::json!({ "before": "Engagement", "after": "ACME" })
        );
        assert_eq!(changes["starts"]["after"], "2026-11-23");
        let (_, none) = s
            .update(Patch {
                name: Some("ACME".into()),
                ..Patch::default()
            })
            .unwrap();
        assert!(none.is_empty());
        for bad in [
            Patch {
                ends: Some("2026-11-01".into()),
                ..Patch::default()
            },
            Patch {
                starts: Some("23/11/2026".into()),
                ..Patch::default()
            },
            Patch {
                expires: Some("2026-02-30".into()),
                ..Patch::default()
            },
            Patch {
                name: Some("a\u{1b}[31m".into()),
                ..Patch::default()
            },
            Patch {
                name: Some("".into()),
                ..Patch::default()
            },
            Patch {
                client: Some("x".repeat(121)),
                ..Patch::default()
            },
            Patch {
                contacts: Some(vec![Contact {
                    name: "a\nb".into(),
                    ..Contact::default()
                }]),
                ..Patch::default()
            },
        ] {
            let p = format!("{bad:?}");
            assert!(matches!(s.update(bad), Err(Error::Invalid(_))), "{p}");
        }
        assert_eq!(s.active().name, "ACME", "a refused update changes nothing");
        let (e, _) = s
            .update(Patch {
                starts: Some("".into()),
                notes: Some("line 1\nline 2".into()),
                ..Patch::default()
            })
            .unwrap();
        assert_eq!(e.starts, None, "an empty string clears a date");
        assert!(
            serde_json::from_str::<Patch>(r#"{"scope": ["x.test"]}"#).is_err(),
            "scope isn't patched here"
        );
    }

    #[test]
    fn rules_are_patched_validated_and_default_for_old_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let defaults = Rules {
            safe_methods: vec!["GET".into()],
            ..Rules::default()
        };
        let (s, _) = Store::open(dir.path(), &Scope::parse("a.test").unwrap(), &defaults).unwrap();
        assert_eq!(
            s.active().rules.safe_methods,
            ["GET"],
            "a new engagement starts from the config"
        );
        let patch = |r: RulesPatch| {
            s.update(Patch {
                rules: Some(r),
                ..Patch::default()
            })
        };
        let (e, changes) = patch(RulesPatch {
            methods: Some(vec!["get".into(), " post ".into()]),
            rate_limit: Some(0.0),
            time_zone: Some("America/Chicago".into()),
            ..RulesPatch::default()
        })
        .unwrap();
        assert_eq!(e.rules.methods, ["GET", "POST"]);
        assert_eq!(e.rules.rate_limit, None, "0 switches the ceiling off");
        assert_eq!(
            changes["rules.time_zone"],
            serde_json::json!({ "before": "UTC", "after": "America/Chicago" })
        );
        assert!(
            matches!(patch(RulesPatch { time_zone: Some("Nowhere/Land".into()), ..RulesPatch::default() }), Err(Error::Invalid(m)) if m.contains("rules of engagement: time zone"))
        );
        assert_eq!(s.active().rules.time_zone, "America/Chicago");

        // A file written before #36 has no rules: it loads with the defaults.
        let id = s.active().id;
        let path = dir.path().join(format!("{id}.json"));
        let mut v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("rules");
        fs::write(&path, v.to_string()).unwrap();
        assert_eq!(load(&path).unwrap().rules, Rules::default());
    }

    #[test]
    fn a_failed_save_changes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let (s, _) = open(dir.path(), "a.test");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
            let r = s.change_scope(|sc| sc.add("b.test"));
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
            if nix_is_root() {
                return; // root ignores the permission bits
            }
            assert!(matches!(r, Err(Error::Io(_))), "{r:?}");
            assert!(!s.scope().contains("b.test"));
            assert_eq!(s.active().scope, ["a.test"]);
        }
    }

    fn nix_is_root() -> bool {
        fs::metadata("/proc/self")
            .map(|m| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    m.uid() == 0
                }
                #[cfg(not(unix))]
                {
                    let _ = m;
                    false
                }
            })
            .unwrap_or(false)
    }

    #[test]
    fn ids_are_file_safe() {
        for name in ["ACME", "../../etc", "  ", "日本語", "a/b\\c"] {
            let id = new_id(name);
            assert!(valid_id(&id), "{name:?} -> {id}");
        }
        assert!(!valid_id("../x") && !valid_id("") && !valid_id("-x") && !valid_id("A"));
    }
}
