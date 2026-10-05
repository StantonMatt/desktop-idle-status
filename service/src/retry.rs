//! Bounded exponential retries, independent for each desktop source.
use std::time::{Duration, Instant};
#[derive(Default)]
pub struct Retry {
    due: Option<Instant>,
    delay: u64,
}
impl Retry {
    pub fn due(&self, now: Instant) -> bool {
        self.due.is_some_and(|due| now >= due)
    }
    pub fn complete(&mut self, now: Instant, success: bool) {
        if success {
            self.due = None;
            self.delay = 0;
        } else {
            self.delay = if self.delay == 0 {
                1
            } else {
                (self.delay * 2).min(30)
            };
            self.due = Some(now + Duration::from_secs(self.delay));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_are_bounded_and_recovery_resets_backoff() {
        let mut retry = Retry::default();
        let mut now = Instant::now();
        assert!(!retry.due(now));
        for seconds in [1, 2, 4, 8, 16, 30, 30] {
            retry.complete(now, false);
            assert!(!retry.due(now + Duration::from_secs(seconds) - Duration::from_millis(1)));
            now += Duration::from_secs(seconds);
            assert!(retry.due(now));
        }
        retry.complete(now, true);
        assert!(!retry.due(now + Duration::from_secs(100)));
        retry.complete(now, false);
        assert!(retry.due(now + Duration::from_secs(1)));
    }
}
