//! Confirmation tokens (#22). When the gate says an action needs
//! confirmation, the server issues a short-lived, single-use token bound
//! to the exact request (a hash of what would be sent). The client sends
//! the same request again with the token. A human gets there by pressing
//! `⏎` a second time (#18) or with `--yes` in the console; an agent (M6)
//! only through an approval.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub type Fingerprint = [u8; 32];

/// The fingerprint of a request, from its canonical bytes.
pub fn fingerprint(canonical: &[u8]) -> Fingerprint {
    Sha256::digest(canonical).into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Never issued, already used, or evicted.
    Unknown,
    Expired,
    /// Issued for a different request.
    Mismatch,
}

impl Refused {
    pub fn message(self) -> &'static str {
        match self {
            Refused::Unknown => "confirmation is unknown or already used",
            Refused::Expired => "confirmation expired; send again to get a new one",
            Refused::Mismatch => "confirmation was issued for a different request",
        }
    }
}

struct Pending {
    fingerprint: Fingerprint,
    expires: Instant,
}

pub struct Confirmations {
    ttl: Duration,
    cap: usize,
    pending: Mutex<HashMap<String, Pending>>,
}

impl Confirmations {
    pub fn new(ttl: Duration, cap: usize) -> Self {
        Confirmations {
            ttl,
            cap: cap.max(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Revoke every outstanding token (the e-stop, #57). Returns how many.
    pub fn clear(&self) -> usize {
        let mut p = self.lock();
        let n = p.len();
        p.clear();
        n
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Issue a token for this request. Fails only if the OS has no randomness.
    pub fn issue(&self, fp: Fingerprint, now: Instant) -> Result<String, getrandom::Error> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut pending = self.lock();
        pending.retain(|_, p| p.expires > now);
        while pending.len() >= self.cap {
            let oldest = pending
                .iter()
                .min_by_key(|(_, p)| p.expires)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => pending.remove(&k),
                None => break,
            };
        }
        pending.insert(
            token.clone(),
            Pending {
                fingerprint: fp,
                expires: now + self.ttl,
            },
        );
        Ok(token)
    }

    /// Use a token. Any attempt consumes it, so a guess or a mismatch
    /// can't be retried against the same token.
    pub fn redeem(&self, token: &str, fp: Fingerprint, now: Instant) -> Result<(), Refused> {
        let Some(p) = self.lock().remove(token) else {
            return Err(Refused::Unknown);
        };
        if p.expires <= now {
            return Err(Refused::Expired);
        }
        if p.fingerprint != fp {
            return Err(Refused::Mismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> Confirmations {
        Confirmations::new(Duration::from_secs(120), 4)
    }

    #[test]
    fn a_token_confirms_its_own_request_once() {
        let c = c();
        let now = Instant::now();
        let fp = fingerprint(b"DELETE /a");
        let t = c.issue(fp, now).unwrap();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(c.redeem(&t, fp, now), Ok(()));
        assert_eq!(c.redeem(&t, fp, now), Err(Refused::Unknown), "reuse");
    }

    #[test]
    fn a_different_request_is_refused_and_burns_the_token() {
        let c = c();
        let now = Instant::now();
        let t = c.issue(fingerprint(b"DELETE /a"), now).unwrap();
        assert_eq!(
            c.redeem(&t, fingerprint(b"DELETE /b"), now),
            Err(Refused::Mismatch)
        );
        assert_eq!(
            c.redeem(&t, fingerprint(b"DELETE /a"), now),
            Err(Refused::Unknown)
        );
    }

    #[test]
    fn tokens_expire() {
        let c = c();
        let now = Instant::now();
        let fp = fingerprint(b"x");
        let t = c.issue(fp, now).unwrap();
        assert_eq!(
            c.redeem(&t, fp, now + Duration::from_secs(120)),
            Err(Refused::Expired)
        );
        let t = c.issue(fp, now).unwrap();
        assert_eq!(c.redeem(&t, fp, now + Duration::from_secs(119)), Ok(()));
    }

    #[test]
    fn unknown_tokens_are_refused_and_tokens_are_distinct() {
        let c = c();
        let now = Instant::now();
        assert_eq!(c.redeem("", fingerprint(b"x"), now), Err(Refused::Unknown));
        let a = c.issue(fingerprint(b"x"), now).unwrap();
        let b = c.issue(fingerprint(b"x"), now).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn pending_tokens_are_bounded() {
        let c = c();
        let start = Instant::now();
        let fp = fingerprint(b"x");
        let first = c.issue(fp, start).unwrap();
        for i in 1..=4 {
            c.issue(fp, start + Duration::from_millis(i)).unwrap();
        }
        assert_eq!(c.lock().len(), 4);
        assert_eq!(
            c.redeem(&first, fp, start),
            Err(Refused::Unknown),
            "oldest evicted"
        );
    }
}
