//! Blocking delays and a monotonic reading, for drivers that need to wait.
//!
//! `embedded_hal` already has [`DelayNs`] for waiting. It has no clock, so
//! that part is ours. Both live behind one trait so a driver carries one extra
//! parameter rather than two.

use core::time::Duration;

use embedded_hal::delay::DelayNs;

/// A blocking delay plus a monotonic reading.
///
/// Passed to the methods that need it rather than stored, so drivers keep the
/// concrete types they had and callers stay unchanged in shape.
pub trait Timer: DelayNs {
    /// Milliseconds from an arbitrary fixed point. Only differences between
    /// readings are meaningful.
    fn now_ms(&self) -> u64;

    /// Wait out `duration`.
    fn sleep(&mut self, duration: Duration) {
        // Microseconds are enough for every wait here -- the shortest is 1us --
        // and keep the value inside a u32 for delays past four seconds.
        self.delay_us(duration.as_micros().min(u32::MAX as u128) as u32);
    }

    /// Wait until `duration` has passed since `since_ms`, if it hasn't already.
    ///
    /// Panels want a minimum interval between sleep-state changes. Waiting the
    /// full interval every time would be simpler but would add that delay to
    /// every display on/off, so keep track of when the last one was.
    fn sleep_until(&mut self, since_ms: u64, duration: Duration) {
        let elapsed = Duration::from_millis(self.now_ms().saturating_sub(since_ms));
        self.sleep(duration.saturating_sub(elapsed));
    }
}

/// The system timer, backed by esp-idf's microsecond clock.
///
/// A bare-metal port would implement [`Timer`] over a hardware timer instead;
/// nothing above this line knows the difference.
#[derive(Copy, Clone, Default)]
pub struct SystemTimer;

impl DelayNs for SystemTimer {
    fn delay_ns(&mut self, ns: u32) {
        esp_idf_svc::hal::delay::Delay::new_default().delay_ns(ns);
    }

    fn delay_us(&mut self, us: u32) {
        esp_idf_svc::hal::delay::Delay::new_default().delay_us(us);
    }

    fn delay_ms(&mut self, ms: u32) {
        esp_idf_svc::hal::delay::Delay::new_default().delay_ms(ms);
    }
}

impl Timer for SystemTimer {
    fn now_ms(&self) -> u64 {
        // Monotonic since boot, in microseconds.
        (unsafe { esp_idf_svc::sys::esp_timer_get_time() } as u64) / 1000
    }
}
