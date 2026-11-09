//! The number typed before a log view command, as in `123g` or `5j`
//! (klogg: `DigitsBuffer` in `abstractlogview.cpp`, GPLv3).
//!
//! Digits accumulate until a command takes them or until
//! [`DIGITS_TIMEOUT`] passes without a new digit. A leading `0` is not a
//! digit: on its own, `0` is a command (start of line).

use std::time::{Duration, Instant};

/// klogg: `DigitsBuffer::DigitsTimeout`.
pub const DIGITS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default)]
pub struct DigitsBuffer {
    digits: String,
    last: Option<Instant>,
}

impl DigitsBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the digits if the last one is older than the timeout.
    fn expire(&mut self, now: Instant) {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) >= DIGITS_TIMEOUT)
        {
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        self.digits.clear();
        self.last = None;
    }

    pub fn is_empty(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.digits.is_empty()
    }

    /// Adds `c` if it is a digit that can be added (`1`–`9`, or `0` after
    /// another digit). Returns whether it was taken.
    pub fn add(&mut self, c: char, now: Instant) -> bool {
        self.expire(now);
        let taken = matches!(c, '1'..='9') || (c == '0' && !self.digits.is_empty());
        if taken {
            self.digits.push(c);
            self.last = Some(now);
        }
        taken
    }

    /// The number typed, if any, and clears it (klogg: `content`, which
    /// returns 0 when empty). Saturates on overflow.
    pub fn take(&mut self, now: Instant) -> Option<u64> {
        self.expire(now);
        if self.digits.is_empty() {
            return None;
        }
        let n = self.digits.parse().unwrap_or(u64::MAX);
        self.reset();
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_and_takes() {
        let t = Instant::now();
        let mut d = DigitsBuffer::new();
        assert!(!d.add('0', t), "a leading 0 is a command");
        assert!(d.add('1', t) && d.add('0', t) && d.add('7', t));
        assert!(!d.add('x', t));
        assert_eq!(d.take(t), Some(107));
        assert_eq!(d.take(t), None);
        assert!(d.is_empty(t));
    }

    #[test]
    fn times_out() {
        let t = Instant::now();
        let mut d = DigitsBuffer::new();
        d.add('4', t);
        // Each digit restarts the timeout.
        d.add('2', t + Duration::from_millis(1500));
        assert_eq!(d.take(t + Duration::from_millis(3000)), Some(42));
        d.add('9', t);
        assert_eq!(d.take(t + DIGITS_TIMEOUT), None);
        // After a timeout, 0 is a command again.
        d.add('5', t);
        assert!(!d.add('0', t + DIGITS_TIMEOUT));
    }

    #[test]
    fn saturates() {
        let t = Instant::now();
        let mut d = DigitsBuffer::new();
        for _ in 0..30 {
            d.add('9', t);
        }
        assert_eq!(d.take(t), Some(u64::MAX));
    }
}
