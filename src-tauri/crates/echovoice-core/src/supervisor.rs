//! Small helpers shared by the bridge supervisor (device-loss detection / reconnect).

/// Exponential backoff with a ceiling, used when retrying a lost audio device.
///
/// Delays are 1s, 2s, 4s, ... capped at `max`. Kept as a pure struct so the reconnect
/// policy can be unit tested without touching WASAPI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backoff {
    initial_secs: u64,
    max_secs: u64,
    current_secs: u64,
    attempts: u32,
}

impl Backoff {
    pub fn new(initial_secs: u64, max_secs: u64) -> Self {
        let initial_secs = initial_secs.max(1);
        let max_secs = max_secs.max(initial_secs);
        Self {
            initial_secs,
            max_secs,
            current_secs: initial_secs,
            attempts: 0,
        }
    }

    /// How long to wait before the next attempt.
    pub fn next_delay_secs(&self) -> u64 {
        self.current_secs
    }

    /// Number of attempts made so far.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Records a failed attempt and doubles the delay up to the ceiling.
    pub fn record_failure(&mut self) {
        self.attempts += 1;
        self.current_secs = (self.current_secs * 2).min(self.max_secs);
    }

    /// Records a successful attempt and resets the delay.
    pub fn record_success(&mut self) {
        self.attempts = 0;
        self.current_secs = self.initial_secs;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(1, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_up_to_the_ceiling() {
        let mut backoff = Backoff::new(1, 8);
        assert_eq!(backoff.next_delay_secs(), 1);
        backoff.record_failure();
        assert_eq!(backoff.next_delay_secs(), 2);
        backoff.record_failure();
        assert_eq!(backoff.next_delay_secs(), 4);
        backoff.record_failure();
        assert_eq!(backoff.next_delay_secs(), 8);
        backoff.record_failure();
        assert_eq!(backoff.next_delay_secs(), 8, "must stay at the ceiling");
        assert_eq!(backoff.attempts(), 4);
    }

    #[test]
    fn success_resets_the_delay() {
        let mut backoff = Backoff::new(1, 8);
        backoff.record_failure();
        backoff.record_failure();
        assert_eq!(backoff.next_delay_secs(), 4);
        backoff.record_success();
        assert_eq!(backoff.next_delay_secs(), 1);
        assert_eq!(backoff.attempts(), 0);
    }

    #[test]
    fn never_waits_less_than_one_second() {
        let backoff = Backoff::new(0, 0);
        assert_eq!(backoff.next_delay_secs(), 1);
    }
}
