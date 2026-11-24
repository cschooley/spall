//! Quad mode (#66, docs/ux.md "Quad mode"): a time-boxed burst for the
//! agent. `quad [seconds]` gives agent tasks 4× the workers (spall's host
//! drives four times as many tasks at once) and 4× their own rate, then it
//! wears off on its own.
//!
//! It never exceeds the engagement: the rate is the task's ×4 capped at the
//! RoE's ceiling (the gate takes the lower of the two as always, and the
//! start says when it's capped), every send is charged to its task's budget
//! as usual, and the gate decides each one. The e-stop ends it at once and
//! no quad starts while it's engaged; a finished task never goes quad.
//!
//! The scorecard is numbers only, by construction: [`Scorecard`] has no
//! string field, so no host, path, payload or client name can reach the
//! shareable card.

use std::sync::Mutex;

pub const DEFAULT_SECS: u64 = 120;
pub const MAX_SECS: u64 = 600;
/// More workers, more rate.
pub const FACTOR: u32 = 4;
/// Tasks spall's host drives at once, outside quad.
pub const WORKERS: u32 = 2;

/// What a burst is measured against: totals in its engagement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub requests: u64,
    pub tokens: u64,
    pub candidates: u64,
}

/// Why a burst ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// It wore off.
    Time,
    /// A human ended it early.
    Stopped,
    /// The e-stop.
    Estop,
}

/// A burst underway.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct On {
    #[serde(skip)]
    pub engagement: String,
    pub started: String,
    pub until: String,
    pub seconds: u64,
    pub workers: u32,
    /// Why the 4× rate is less than 4× (the RoE's ceiling), if it is.
    pub capped: Option<String>,
    #[serde(skip)]
    pub base: Tally,
    /// The tasks live when it started: their logs get its start and end.
    #[serde(skip)]
    pub tasks: Vec<String>,
}

/// A burst's results. Numbers only, by construction: no field can carry an
/// engagement's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Scorecard {
    pub seconds: u64,
    pub workers: u32,
    pub requests: u64,
    pub tokens: u64,
    pub candidates: u64,
    pub ended: Ended,
}

fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn tokens_text(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

impl Scorecard {
    /// The shareable card: made of this struct's numbers and fixed words.
    pub fn card(&self) -> String {
        format!(
            "QUAD {} · {}× agents · {} requests · {} tokens · {} candidates{} · spall",
            mmss(self.seconds),
            self.workers,
            self.requests,
            tokens_text(self.tokens),
            self.candidates,
            match self.ended {
                Ended::Time => "",
                Ended::Stopped => " · ended early",
                Ended::Estop => " · e-stopped",
            }
        )
    }
}

#[derive(Default)]
struct Inner {
    on: Option<On>,
    last: Option<Scorecard>,
}

/// The one burst there can be, and the last one's scorecard.
#[derive(Default)]
pub struct Boost {
    inner: Mutex<Inner>,
}

/// Seconds asked for, checked: default 120, at most 600.
pub fn seconds(asked: Option<u64>) -> Result<u64, String> {
    match asked.unwrap_or(DEFAULT_SECS) {
        s @ 1..=MAX_SECS => Ok(s),
        _ => Err(format!(
            "quad lasts 1 to {MAX_SECS} seconds (default {DEFAULT_SECS})"
        )),
    }
}

impl Boost {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Start a burst; `Err` if one is on.
    pub fn start(&self, on: On) -> Result<On, String> {
        let mut g = self.lock();
        if g.on.is_some() {
            return Err("quad is already on".into());
        }
        g.on = Some(on.clone());
        Ok(on)
    }

    /// The burst underway, unless it has worn off by `now` (the caller ends
    /// it then, with [`Boost::end`]).
    pub fn current(&self) -> Option<On> {
        self.lock().on.clone()
    }

    /// Whether a burst is on for `engagement` at `now` and not past its end.
    pub fn active_for(&self, engagement: &str, now: jiff::Timestamp) -> bool {
        self.lock().on.as_ref().is_some_and(|o| {
            o.engagement == engagement && o.until.parse::<jiff::Timestamp>().is_ok_and(|u| now < u)
        })
    }

    /// End the burst underway, scored against `now_tally`, keeping the
    /// scorecard; `None` if there was none.
    pub fn end(
        &self,
        ended: Ended,
        now: jiff::Timestamp,
        now_tally: Tally,
    ) -> Option<(On, Scorecard)> {
        let mut g = self.lock();
        let on = g.on.take()?;
        let started = on.started.parse::<jiff::Timestamp>().unwrap_or(now);
        let until = on.until.parse::<jiff::Timestamp>().unwrap_or(now);
        let stop = if ended == Ended::Time {
            until
        } else {
            now.min(until)
        };
        let seconds = u64::try_from(stop.duration_since(started).as_secs()).unwrap_or(0);
        let card = Scorecard {
            seconds,
            workers: on.workers,
            requests: now_tally.requests.saturating_sub(on.base.requests),
            tokens: now_tally.tokens.saturating_sub(on.base.tokens),
            candidates: now_tally.candidates.saturating_sub(on.base.candidates),
            ended,
        };
        g.last = Some(card);
        Some((on, card))
    }

    pub fn last(&self) -> Option<Scorecard> {
        self.lock().last
    }
}

/// The 4× rate for a task allowed `task_rate`, and whether the RoE's
/// `ceiling` caps it below that.
pub fn boosted(task_rate: Option<f64>, ceiling: Option<f64>) -> (Option<f64>, bool) {
    let want = task_rate.map(|r| r * f64::from(FACTOR));
    match (want, ceiling) {
        (Some(w), Some(c)) if w > c => (Some(c), true),
        (None, Some(c)) => (Some(c), true),
        (w, _) => (w, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(base: Tally) -> On {
        On {
            engagement: "evilcorp-q4".into(),
            started: "2026-11-24T10:00:00Z".into(),
            until: "2026-11-24T10:02:00Z".into(),
            seconds: 120,
            workers: WORKERS * FACTOR,
            capped: None,
            base,
            tasks: vec!["T-1".into()],
        }
    }

    #[test]
    fn seconds_default_and_bounds() {
        assert_eq!(seconds(None), Ok(120));
        assert_eq!(seconds(Some(600)), Ok(600));
        assert!(seconds(Some(0)).is_err() && seconds(Some(601)).is_err());
    }

    #[test]
    fn one_burst_at_a_time_scored_from_where_it_started() {
        let b = Boost::default();
        let base = Tally {
            requests: 10,
            tokens: 5_000,
            candidates: 2,
        };
        b.start(on(base)).unwrap();
        assert!(b.start(on(base)).is_err());
        let t = |s: &str| s.parse::<jiff::Timestamp>().unwrap();
        assert!(b.active_for("evilcorp-q4", t("2026-11-24T10:01:00Z")));
        assert!(
            !b.active_for("evilcorp-q4", t("2026-11-24T10:02:00Z")),
            "worn off"
        );
        assert!(!b.active_for("other", t("2026-11-24T10:01:00Z")));
        let (_, card) = b
            .end(
                Ended::Estop,
                t("2026-11-24T10:00:45Z"),
                Tally {
                    requests: 147,
                    tokens: 417_000,
                    candidates: 5,
                },
            )
            .unwrap();
        assert_eq!(
            (card.seconds, card.requests, card.tokens, card.candidates),
            (45, 137, 412_000, 3)
        );
        assert_eq!(
            card.card(),
            "QUAD 0:45 · 8× agents · 137 requests · 412.0k tokens · 3 candidates · e-stopped · spall"
        );
        assert_eq!(b.last(), Some(card));
        assert!(
            b.current().is_none()
                && b.end(Ended::Time, t("2026-11-24T10:03:00Z"), base)
                    .is_none()
        );
    }

    #[test]
    fn a_worn_off_burst_counts_its_whole_time_only() {
        let b = Boost::default();
        b.start(on(Tally::default())).unwrap();
        let late = "2026-11-24T10:09:00Z".parse().unwrap();
        let (_, card) = b.end(Ended::Time, late, Tally::default()).unwrap();
        assert_eq!(card.seconds, 120);
        assert_eq!(
            card.card(),
            "QUAD 2:00 · 8× agents · 0 requests · 0 tokens · 0 candidates · spall"
        );
    }

    #[test]
    fn the_card_is_numbers_and_fixed_words_whatever_it_scores() {
        // No string field exists to leak through; this checks the words.
        let fixed = [
            "QUAD",
            "·",
            "agents",
            "requests",
            "tokens",
            "candidates",
            "ended",
            "early",
            "e-stopped",
            "spall",
        ];
        for ended in [Ended::Time, Ended::Stopped, Ended::Estop] {
            for n in [0u64, 7, 999, 1_000, 12_345_678, u64::MAX / 2] {
                let c = Scorecard {
                    seconds: n % 601,
                    workers: 8,
                    requests: n,
                    tokens: n,
                    candidates: n,
                    ended,
                };
                for word in c.card().split_whitespace() {
                    let numeric = word
                        .trim_end_matches(['k', 'M', '×'])
                        .chars()
                        .all(|ch| ch.is_ascii_digit() || ch == '.' || ch == ':');
                    assert!(numeric || fixed.contains(&word), "{word:?} in {}", c.card());
                }
            }
        }
    }

    #[test]
    fn four_times_the_rate_never_past_the_ceiling() {
        assert_eq!(boosted(Some(1.0), Some(10.0)), (Some(4.0), false));
        assert_eq!(boosted(Some(5.0), Some(10.0)), (Some(10.0), true));
        assert_eq!(boosted(None, Some(10.0)), (Some(10.0), true));
        assert_eq!(boosted(Some(2.0), None), (Some(8.0), false));
        assert_eq!(boosted(None, None), (None, false));
    }
}
