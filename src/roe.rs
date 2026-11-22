//! Rules of engagement (#36): the limits an engagement puts on every action,
//! as data on the engagement. The gate (#22) reads them; M6 task
//! constraints are layered on top and can only narrow them (a layer can
//! deny, never allow what another layer denies).
//!
//! Times are evaluated in the rules' IANA time zone, with the tz database
//! bundled (the image has no zoneinfo).

use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

const DAYS: [Day; 7] = [
    Day::Mon,
    Day::Tue,
    Day::Wed,
    Day::Thu,
    Day::Fri,
    Day::Sat,
    Day::Sun,
];

impl Day {
    fn of(w: Weekday) -> Day {
        DAYS[usize::from(w.to_monday_zero_offset() as u8)]
    }
    fn index(self) -> usize {
        DAYS.iter().position(|d| *d == self).unwrap_or(0)
    }
    fn prev(self) -> Day {
        DAYS[(self.index() + 6) % 7]
    }
    fn name(self) -> &'static str {
        ["mon", "tue", "wed", "thu", "fri", "sat", "sun"][self.index()]
    }
}

/// A weekly testing window: `days` from `start` to `end` (`HH:MM`, end may
/// be `24:00`). An end before the start runs past midnight into the next day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub days: Vec<Day>,
    pub start: String,
    pub end: String,
}

/// A no-test window, `from` to `to` as `YYYY-MM-DDTHH:MM` local times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blackout {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Methods that may be sent at all; empty means any.
    #[serde(default)]
    pub methods: Vec<String>,
    /// Methods sent without confirmation (others take two presses).
    #[serde(default = "default_safe")]
    pub safe_methods: Vec<String>,
    /// Sends per second, across the engagement; `None` is no ceiling.
    #[serde(default = "default_rate")]
    pub rate_limit: Option<f64>,
    #[serde(default = "default_tz")]
    pub time_zone: String,
    /// Weekly testing windows; empty means any time.
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub blackouts: Vec<Blackout>,
    /// Agents may act (M6). Off unless the engagement says so.
    #[serde(default)]
    pub agents_allowed: bool,
    /// Destructive-class actions may run.
    #[serde(default)]
    pub destructive_allowed: bool,
}

fn default_safe() -> Vec<String> {
    vec!["GET".into(), "HEAD".into(), "OPTIONS".into()]
}
fn default_rate() -> Option<f64> {
    Some(10.0)
}
fn default_tz() -> String {
    "UTC".into()
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            methods: Vec::new(),
            safe_methods: default_safe(),
            rate_limit: default_rate(),
            time_zone: default_tz(),
            windows: Vec::new(),
            blackouts: Vec::new(),
            agents_allowed: false,
            destructive_allowed: false,
        }
    }
}

/// Who is acting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Human,
    Agent,
}

/// What kind of action: sending a request, or something destructive
/// (M6: fuzzing, brute force, anything the registry marks destructive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Send,
    Destructive,
}

/// The engagement's dates (`YYYY-MM-DD`, inclusive), checked with the rules.
#[derive(Debug, Clone, Default)]
pub struct Period {
    pub starts: Option<String>,
    pub ends: Option<String>,
    pub expires: Option<String>,
}

const MAX_RATE: f64 = 1000.0;

fn minutes(s: &str, allow_24: bool) -> Result<u16, String> {
    let bad = || format!("want HH:MM, got {s:?}");
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    if h.len() != 2 || m.len() != 2 {
        return Err(bad());
    }
    let h: u16 = h.parse().map_err(|_| bad())?;
    let m: u16 = m.parse().map_err(|_| bad())?;
    if (h == 24 && m == 0 && allow_24) || (h < 24 && m < 60) {
        Ok(h * 60 + m)
    } else {
        Err(bad())
    }
}

fn local(s: &str) -> Result<DateTime, String> {
    let ok = s.len() == 16 && s.as_bytes()[10] == b'T';
    ok.then(|| s.parse::<DateTime>().ok())
        .flatten()
        .ok_or_else(|| format!("want YYYY-MM-DDTHH:MM, got {s:?}"))
}

fn token(m: &str) -> bool {
    !m.is_empty()
        && m.len() <= 32
        && m.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Days as a short phrase: `mon-fri`, `sat,sun`, `daily`.
pub fn days_text(days: &[Day]) -> String {
    let mut on = [false; 7];
    for d in days {
        on[d.index()] = true;
    }
    if on.iter().all(|x| *x) {
        return "daily".into();
    }
    let mut parts = Vec::new();
    let mut i = 0;
    while i < 7 {
        if !on[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i + 1 < 7 && on[i + 1] {
            i += 1;
        }
        parts.push(match i - start {
            0 => DAYS[start].name().to_string(),
            1 => format!("{},{}", DAYS[start].name(), DAYS[i].name()),
            _ => format!("{}-{}", DAYS[start].name(), DAYS[i].name()),
        });
        i += 1;
    }
    parts.join(",")
}

impl Window {
    fn contains(&self, day: Day, minute: u16) -> bool {
        let (Ok(s), Ok(e)) = (minutes(&self.start, false), minutes(&self.end, true)) else {
            return false;
        };
        if s < e {
            self.days.contains(&day) && minute >= s && minute < e
        } else {
            (self.days.contains(&day) && minute >= s)
                || (self.days.contains(&day.prev()) && minute < e)
        }
    }

    pub fn text(&self) -> String {
        format!("{} {}-{}", days_text(&self.days), self.start, self.end)
    }
}

impl Rules {
    pub fn validate(&self) -> Result<(), String> {
        for (name, list) in [
            ("methods", &self.methods),
            ("safe methods", &self.safe_methods),
        ] {
            if list.len() > 32 || !list.iter().all(|m| token(m)) {
                return Err(format!(
                    "{name}: uppercase HTTP method names, comma-separated"
                ));
            }
        }
        if let Some(r) = self.rate_limit {
            if !(r > 0.0 && r <= MAX_RATE) {
                return Err(format!(
                    "rate limit: more than 0 and at most {MAX_RATE} per second (or off)"
                ));
            }
        }
        self.tz()?;
        if self.windows.len() > 50 || self.blackouts.len() > 50 {
            return Err("at most 50 testing windows and 50 no-test windows".into());
        }
        for w in &self.windows {
            if w.days.is_empty() {
                return Err("testing window: no days".into());
            }
            let s = minutes(&w.start, false).map_err(|e| format!("testing window start: {e}"))?;
            let e = minutes(&w.end, true).map_err(|e| format!("testing window end: {e}"))?;
            if s == e {
                return Err(format!(
                    "testing window {}: starts and ends at the same time",
                    w.text()
                ));
            }
        }
        for b in &self.blackouts {
            let from = local(&b.from).map_err(|e| format!("no-test window from: {e}"))?;
            let to = local(&b.to).map_err(|e| format!("no-test window to: {e}"))?;
            if to <= from {
                return Err(format!(
                    "no-test window {} to {}: ends before it starts",
                    b.from, b.to
                ));
            }
            if b.note.chars().count() > 200 || b.note.chars().any(char::is_control) {
                return Err("no-test window note: at most 200 characters, one line".into());
            }
        }
        Ok(())
    }

    fn tz(&self) -> Result<TimeZone, String> {
        TimeZone::get(&self.time_zone).map_err(|_| {
            format!(
                "time zone {:?}: not an IANA zone (like Europe/Berlin or UTC)",
                self.time_zone
            )
        })
    }

    pub fn is_safe(&self, method: &str) -> bool {
        self.safe_methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method))
    }

    /// Whether the engagement's dates, no-test windows and testing windows
    /// allow acting at `now`. `Err` says why not.
    pub fn check_time(&self, period: &Period, now: Timestamp) -> Result<(), String> {
        let tz = self.tz()?;
        let zone = &self.time_zone;
        let here = now.to_zoned(tz.clone());
        let today = here.date();
        let date = |s: &Option<String>| s.as_deref().and_then(|d| d.parse::<Date>().ok());
        if let Some(starts) = date(&period.starts) {
            if today < starts {
                return Err(format!(
                    "the engagement starts on {starts} ({zone}); nothing may be sent before then"
                ));
            }
        }
        if let Some(ends) = date(&period.ends) {
            if today > ends {
                return Err(format!("the engagement ended on {ends} ({zone})"));
            }
        }
        if let Some(expires) = date(&period.expires) {
            if today > expires {
                return Err(format!(
                    "the authorization to test expired on {expires} ({zone})"
                ));
            }
        }
        for b in &self.blackouts {
            let (Ok(from), Ok(to)) = (local(&b.from), local(&b.to)) else {
                continue;
            };
            let at = |dt: DateTime| tz.to_ambiguous_timestamp(dt).compatible().ok();
            if let (Some(from), Some(to)) = (at(from), at(to)) {
                if now >= from && now < to {
                    let note = if b.note.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", b.note)
                    };
                    return Err(format!(
                        "in a no-test window until {} ({zone}){note}",
                        b.to.replace('T', " ")
                    ));
                }
            }
        }
        if !self.windows.is_empty() {
            let t: Time = here.time();
            let minute = u16::from(t.hour() as u8) * 60 + u16::from(t.minute() as u8);
            let day = Day::of(here.weekday());
            if !self.windows.iter().any(|w| w.contains(day, minute)) {
                let list: Vec<String> = self.windows.iter().map(Window::text).collect();
                return Err(format!(
                    "outside the testing windows ({} {zone}; it is {} {:02}:{:02} there)",
                    list.join("; "),
                    day.name(),
                    t.hour(),
                    t.minute()
                ));
            }
        }
        Ok(())
    }

    /// Whether this actor may take this kind of action with this method.
    pub fn check_action(&self, actor: Actor, class: Class, method: &str) -> Result<(), String> {
        if actor == Actor::Agent && !self.agents_allowed {
            return Err("the rules of engagement don't allow agents to act".into());
        }
        if class == Class::Destructive && !self.destructive_allowed {
            return Err("the rules of engagement don't allow destructive actions".into());
        }
        if !self.methods.is_empty() && !self.methods.iter().any(|m| m.eq_ignore_ascii_case(method))
        {
            return Err(format!(
                "{method} isn't allowed by the rules of engagement (allowed: {})",
                self.methods.join(", ")
            ));
        }
        Ok(())
    }
}

/// Every layer must allow (the engagement's rules, then a task's): a
/// layer can only narrow what the ones before it allow.
pub fn check_layers(
    layers: &[&Rules],
    period: &Period,
    now: Timestamp,
    actor: Actor,
    class: Class,
    method: &str,
) -> Result<(), String> {
    for rules in layers {
        rules.check_time(period, now)?;
        rules.check_action(actor, class, method)?;
    }
    Ok(())
}

/// Safe (no confirmation) only if every layer says so.
pub fn safe_in_all(layers: &[&Rules], method: &str) -> bool {
    layers.iter().all(|r| r.is_safe(method))
}

/// The lowest rate ceiling of the layers, if any has one.
pub fn rate_of(layers: &[&Rules]) -> Option<f64> {
    layers
        .iter()
        .filter_map(|r| r.rate_limit)
        .fold(None, |acc: Option<f64>, r| {
            Some(acc.map_or(r, |a| a.min(r)))
        })
}

/// A token bucket: bursts up to the rate (at least 1), refilled at the rate.
#[derive(Debug, Default)]
pub struct Limiter {
    tokens: f64,
    last: Option<Instant>,
}

impl Limiter {
    /// Take one send at `now`; `Err` is how long until one is allowed.
    pub fn take(&mut self, rate: Option<f64>, now: Instant) -> Result<(), Duration> {
        let Some(rate) = rate.filter(|r| *r > 0.0) else {
            return Ok(());
        };
        let burst = rate.max(1.0);
        self.tokens = match self.last {
            None => burst,
            Some(last) => {
                (self.tokens + now.saturating_duration_since(last).as_secs_f64() * rate).min(burst)
            }
        };
        self.last = Some(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / rate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn rules(tz: &str) -> Rules {
        Rules {
            time_zone: tz.into(),
            ..Rules::default()
        }
    }

    fn win(days: &[Day], start: &str, end: &str) -> Window {
        Window {
            days: days.to_vec(),
            start: start.into(),
            end: end.into(),
        }
    }

    const WEEK: [Day; 5] = [Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri];

    #[test]
    fn defaults_are_conservative() {
        let r = Rules::default();
        r.validate().unwrap();
        assert!(r.is_safe("get") && !r.is_safe("POST"));
        assert_eq!(r.rate_limit, Some(10.0));
        assert!(!r.agents_allowed && !r.destructive_allowed);
        let parsed: Rules = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, r, "missing fields take the defaults");
    }

    #[test]
    fn testing_windows_in_the_rules_time_zone() {
        let mut r = rules("America/Chicago");
        r.windows = vec![win(&WEEK, "09:00", "17:00")];
        // 2026-11-23 is a Monday. 15:00Z is 09:00 in Chicago (CST, -6).
        assert!(r
            .check_time(&Period::default(), at("2026-11-23T15:00:00Z"))
            .is_ok());
        let early = r
            .check_time(&Period::default(), at("2026-11-23T14:59:00Z"))
            .unwrap_err();
        assert!(early.contains("outside the testing windows (mon-fri 09:00-17:00 America/Chicago; it is mon 08:59 there)"), "{early}");
        assert!(
            r.check_time(&Period::default(), at("2026-11-23T23:00:00Z"))
                .is_err(),
            "17:00 is the end"
        );
        // Saturday morning in Chicago.
        assert!(r
            .check_time(&Period::default(), at("2026-11-28T16:00:00Z"))
            .is_err());
    }

    #[test]
    fn windows_follow_daylight_saving() {
        let mut r = rules("America/New_York");
        r.windows = vec![win(&[Day::Sun], "09:00", "10:00")];
        // 2026-03-08: US clocks spring forward. 09:30 EDT is 13:30Z, not 14:30Z.
        assert!(r
            .check_time(&Period::default(), at("2026-03-08T13:30:00Z"))
            .is_ok());
        assert!(r
            .check_time(&Period::default(), at("2026-03-08T14:30:00Z"))
            .is_err());
        // A week earlier, still EST: 09:30 is 14:30Z.
        assert!(r
            .check_time(&Period::default(), at("2026-03-01T14:30:00Z"))
            .is_ok());
    }

    #[test]
    fn overnight_windows_run_into_the_next_day() {
        let mut r = rules("UTC");
        r.windows = vec![win(&[Day::Fri], "22:00", "06:00")];
        let ok = |s| r.check_time(&Period::default(), at(s)).is_ok();
        assert!(ok("2026-11-27T23:00:00Z"), "friday night");
        assert!(
            ok("2026-11-28T05:59:00Z"),
            "saturday early, from friday's window"
        );
        assert!(!ok("2026-11-28T06:00:00Z"));
        assert!(!ok("2026-11-28T23:00:00Z"), "saturday night isn't listed");
        assert!(
            !ok("2026-11-27T05:00:00Z"),
            "friday early belongs to thursday"
        );
        let whole = Rules {
            windows: vec![win(&[Day::Sat], "00:00", "24:00")],
            ..rules("UTC")
        };
        assert!(whole
            .check_time(&Period::default(), at("2026-11-28T23:59:00Z"))
            .is_ok());
    }

    #[test]
    fn no_test_windows_and_engagement_dates() {
        let mut r = rules("Europe/Berlin");
        r.blackouts = vec![Blackout {
            from: "2026-11-26T18:00".into(),
            to: "2026-11-27T08:00".into(),
            note: "release freeze".into(),
        }];
        let p = Period::default();
        // 18:00 Berlin (CET, +1) is 17:00Z.
        let e = r.check_time(&p, at("2026-11-26T17:00:00Z")).unwrap_err();
        assert_eq!(
            e,
            "in a no-test window until 2026-11-27 08:00 (Europe/Berlin): release freeze"
        );
        assert!(r.check_time(&p, at("2026-11-26T16:59:00Z")).is_ok());
        assert!(
            r.check_time(&p, at("2026-11-27T07:00:00Z")).is_ok(),
            "08:00 Berlin ends it"
        );

        let period = Period {
            starts: Some("2026-11-23".into()),
            ends: Some("2026-12-04".into()),
            expires: Some("2026-11-30".into()),
        };
        let r = rules("Pacific/Auckland");
        // 2026-11-22T12:00Z is already 2026-11-23 01:00 in Auckland.
        assert!(r.check_time(&period, at("2026-11-22T12:00:00Z")).is_ok());
        assert!(r
            .check_time(&period, at("2026-11-22T10:00:00Z"))
            .unwrap_err()
            .contains("starts on 2026-11-23"));
        assert!(
            r.check_time(&period, at("2026-11-30T10:00:00Z")).is_ok(),
            "the expiry day itself is allowed"
        );
        let e = r
            .check_time(&period, at("2026-11-30T12:00:00Z"))
            .unwrap_err();
        assert!(
            e.contains("authorization to test expired on 2026-11-30"),
            "{e}"
        );
    }

    #[test]
    fn actions_methods_agents_and_destructive() {
        let mut r = Rules::default();
        assert!(
            r.check_action(Actor::Human, Class::Send, "PROPFIND")
                .is_ok(),
            "no method list: any method"
        );
        assert!(r
            .check_action(Actor::Agent, Class::Send, "GET")
            .unwrap_err()
            .contains("agents"));
        assert!(r
            .check_action(Actor::Human, Class::Destructive, "GET")
            .unwrap_err()
            .contains("destructive"));
        r.methods = vec!["GET".into(), "POST".into()];
        r.agents_allowed = true;
        assert_eq!(
            r.check_action(Actor::Agent, Class::Send, "DELETE")
                .unwrap_err(),
            "DELETE isn't allowed by the rules of engagement (allowed: GET, POST)"
        );
        assert!(r.check_action(Actor::Agent, Class::Send, "post").is_ok());
    }

    #[test]
    fn validation() {
        Rules {
            windows: vec![win(&WEEK, "22:00", "06:00")],
            ..rules("Asia/Tokyo")
        }
        .validate()
        .unwrap();
        let bad = [
            rules("Mars/Olympus"),
            Rules {
                rate_limit: Some(0.0),
                ..Rules::default()
            },
            Rules {
                rate_limit: Some(f64::NAN),
                ..Rules::default()
            },
            Rules {
                rate_limit: Some(5000.0),
                ..Rules::default()
            },
            Rules {
                methods: vec!["get".into()],
                ..Rules::default()
            },
            Rules {
                safe_methods: vec!["G T".into()],
                ..Rules::default()
            },
            Rules {
                windows: vec![win(&[], "09:00", "17:00")],
                ..Rules::default()
            },
            Rules {
                windows: vec![win(&WEEK, "9:00", "17:00")],
                ..Rules::default()
            },
            Rules {
                windows: vec![win(&WEEK, "24:00", "17:00")],
                ..Rules::default()
            },
            Rules {
                windows: vec![win(&WEEK, "09:00", "09:00")],
                ..Rules::default()
            },
            Rules {
                blackouts: vec![Blackout {
                    from: "2026-11-27T08:00".into(),
                    to: "2026-11-26T08:00".into(),
                    note: String::new(),
                }],
                ..Rules::default()
            },
            Rules {
                blackouts: vec![Blackout {
                    from: "2026-11-27".into(),
                    to: "2026-11-28".into(),
                    note: String::new(),
                }],
                ..Rules::default()
            },
            Rules {
                blackouts: vec![Blackout {
                    from: "2026-11-27T08:00".into(),
                    to: "2026-11-28T08:00".into(),
                    note: "a\nb".into(),
                }],
                ..Rules::default()
            },
        ];
        for r in bad {
            assert!(r.validate().is_err(), "{r:?}");
        }
        assert!(serde_json::from_str::<Rules>(
            r#"{"windows":[{"days":["funday"],"start":"09:00","end":"10:00"}]}"#
        )
        .is_err());
    }

    #[test]
    fn days_read_as_ranges() {
        assert_eq!(days_text(&WEEK), "mon-fri");
        assert_eq!(days_text(&[Day::Sat, Day::Sun]), "sat,sun");
        assert_eq!(days_text(&DAYS), "daily");
        assert_eq!(
            days_text(&[Day::Mon, Day::Wed, Day::Thu, Day::Fri]),
            "mon,wed-fri"
        );
    }

    #[test]
    fn layers_only_narrow() {
        let base = Rules {
            agents_allowed: true,
            methods: vec!["GET".into(), "POST".into()],
            rate_limit: Some(10.0),
            ..Rules::default()
        };
        let task = Rules {
            agents_allowed: true,
            destructive_allowed: true, // can't widen: the base still says no
            methods: vec!["GET".into(), "DELETE".into()],
            safe_methods: vec!["GET".into(), "POST".into()],
            rate_limit: Some(2.0),
            windows: vec![win(&WEEK, "09:00", "17:00")],
            ..Rules::default()
        };
        let p = Period::default();
        let times = [
            "2026-11-23T10:00:00Z",
            "2026-11-23T20:00:00Z",
            "2026-11-28T10:00:00Z",
        ];
        for t in times {
            for actor in [Actor::Human, Actor::Agent] {
                for class in [Class::Send, Class::Destructive] {
                    for m in ["GET", "POST", "DELETE", "PUT"] {
                        let both =
                            check_layers(&[&base, &task], &p, at(t), actor, class, m).is_ok();
                        let alone = check_layers(&[&base], &p, at(t), actor, class, m).is_ok();
                        assert!(
                            !both || alone,
                            "narrowed allowed what the base denies: {t} {actor:?} {class:?} {m}"
                        );
                    }
                }
            }
        }
        assert!(check_layers(
            &[&base, &task],
            &p,
            at(times[0]),
            Actor::Agent,
            Class::Send,
            "DELETE"
        )
        .is_err());
        assert!(check_layers(
            &[&base, &task],
            &p,
            at(times[0]),
            Actor::Agent,
            Class::Destructive,
            "GET"
        )
        .is_err());
        // The task narrows: outside its window, and POST isn't in its methods.
        let layered = |t: &str, m: &str| {
            check_layers(&[&base, &task], &p, at(t), Actor::Agent, Class::Send, m)
        };
        assert!(check_layers(&[&base], &p, at(times[1]), Actor::Agent, Class::Send, "GET").is_ok());
        assert!(layered(times[1], "GET")
            .unwrap_err()
            .contains("outside the testing windows"));
        assert!(layered(times[0], "POST")
            .unwrap_err()
            .contains("POST isn't allowed"));
        assert!(layered(times[0], "GET").is_ok());
        assert!(
            !safe_in_all(&[&base, &task], "POST"),
            "the base still wants POST confirmed"
        );
        assert_eq!(rate_of(&[&base, &task]), Some(2.0));
        assert_eq!(
            rate_of(&[&Rules {
                rate_limit: None,
                ..Rules::default()
            }]),
            None
        );
    }

    #[test]
    fn the_limiter_allows_a_burst_then_paces() {
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for _ in 0..2 {
            assert!(l.take(Some(2.0), t0).is_ok());
        }
        let wait = l.take(Some(2.0), t0).unwrap_err();
        assert!((wait.as_secs_f64() - 0.5).abs() < 1e-6, "{wait:?}");
        assert!(l.take(Some(2.0), t0 + Duration::from_millis(500)).is_ok());
        let mut slow = Limiter::default();
        assert!(slow.take(Some(0.5), t0).is_ok(), "a burst of at least one");
        assert!(slow.take(Some(0.5), t0 + Duration::from_secs(1)).is_err());
        assert!(slow.take(Some(0.5), t0 + Duration::from_secs(2)).is_ok());
        assert!(Limiter::default().take(None, t0).is_ok());
    }
}
