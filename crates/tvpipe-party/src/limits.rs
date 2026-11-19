//! Rate limits: a token bucket per socket, a sliding counter per address.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// `rate` tokens per second, at most `burst` saved up.
#[derive(Debug, Clone)]
pub(crate) struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(rate: f64, burst: f64, now: Instant) -> TokenBucket {
        TokenBucket {
            rate,
            burst,
            tokens: burst,
            last: now,
        }
    }

    /// Takes one token; `false` when there is none.
    pub fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        self.last = self.last.max(now);
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// At most `limit` events per key within `window` (failed joins per
/// address).
#[derive(Debug)]
pub(crate) struct WindowCounter {
    limit: usize,
    window: Duration,
    hits: HashMap<String, Vec<Instant>>,
}

impl WindowCounter {
    pub fn new(limit: usize, window: Duration) -> WindowCounter {
        WindowCounter {
            limit,
            window,
            hits: HashMap::new(),
        }
    }

    fn recent(&mut self, key: &str, now: Instant) -> usize {
        let window = self.window;
        match self.hits.get_mut(key) {
            Some(list) => {
                list.retain(|&t| now.saturating_duration_since(t) < window);
                list.len()
            }
            None => 0,
        }
    }

    /// Counts one event; `false` when over the limit (then not counted).
    pub fn hit(&mut self, key: &str, now: Instant) -> bool {
        if self.recent(key, now) >= self.limit {
            return false;
        }
        if self.hits.len() > 10_000 {
            let window = self.window;
            self.hits
                .retain(|_, v| v.iter().any(|&t| now.saturating_duration_since(t) < window));
        }
        self.hits.entry(key.to_owned()).or_default().push(now);
        true
    }

    /// Whether `key` is over the limit now (without counting).
    pub fn over(&mut self, key: &str, now: Instant) -> bool {
        self.recent(key, now) >= self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(2.0, 3.0, t0);
        assert!(b.take(t0) && b.take(t0) && b.take(t0));
        assert!(!b.take(t0));
        assert!(b.take(t0 + Duration::from_millis(500)));
        assert!(!b.take(t0 + Duration::from_millis(500)));
    }

    #[test]
    fn window() {
        let t0 = Instant::now();
        let mut w = WindowCounter::new(2, Duration::from_secs(60));
        assert!(w.hit("a", t0) && w.hit("a", t0));
        assert!(w.over("a", t0) && !w.hit("a", t0));
        assert!(!w.over("b", t0));
        assert!(!w.over("a", t0 + Duration::from_secs(61)));
    }
}
