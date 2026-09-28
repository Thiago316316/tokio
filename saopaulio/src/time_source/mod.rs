//! `Instant` ↔ `u64` tick conversion.
//!
//! The wheel counts milliseconds from `elapsed = 0`, which is not a time
//! anything else in the world understands. `TimeSource` pins that zero to a
//! real `Instant` and converts in both directions.
//!
//! Tokio does the same job in `runtime/time/source.rs`, and keeping the wheel
//! in integer ticks rather than `Instant`s is also what makes a mockable clock
//! (`time::pause()`) possible later.

use std::time::Instant;

pub struct TimeSource {
    start: Instant,
}

impl TimeSource {
    /// Pins tick 0 to this moment. Call once, when the runtime is built.
    pub fn new() -> Self {
        TimeSource { start: Instant::now() }
    }

    /// The current tick. Rounds **down** — "how many whole milliseconds have
    /// definitely elapsed".
    pub fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// The tick a deadline belongs to. Rounds **up**.
    ///
    /// `expire` is inclusive (`deadline <= now` fires), so rounding down would
    /// let `sleep(1.4ms)` complete at 1.0ms — earlier than asked. Rounding up
    /// puts it at tick 2: 0.6ms late, which is the advertised cost of 1ms
    /// granularity. Late is a documented trade-off; early is a broken promise.
    pub fn deadline_to_tick(&self, when: Instant) -> u64 {
        let d = when.saturating_duration_since(self.start);
        d.as_nanos().div_ceil(1_000_000) as u64
    }
}

impl Default for TimeSource {
    fn default() -> Self {
        Self::new()
    }
}
