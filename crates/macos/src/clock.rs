//! Darwin thread CPU clock, kept separate from elapsed callback/presentation time.
use std::time::Duration;
#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}
unsafe extern "C" {
    fn clock_gettime(clock: u32, time: *mut Timespec) -> i32;
}
fn now() -> Option<Duration> {
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // SAFETY: Darwin's clockid_t is unsigned int, timespec uses two 64-bit longs
    // on supported macOS targets, and CLOCK_THREAD_CPUTIME_ID is 16.
    if unsafe { clock_gettime(16, &mut time) } != 0
        || time.seconds < 0
        || !(0..1_000_000_000).contains(&time.nanoseconds)
    {
        return None;
    }
    Some(Duration::new(time.seconds as u64, time.nanoseconds as u32))
}
pub struct Span(Option<Duration>);
impl Span {
    pub fn start() -> Self {
        Self(now())
    }
    pub fn elapsed(&self) -> Option<Duration> {
        now()?.checked_sub(self.0?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thread_clock_measures_work() {
        let span = Span::start();
        for i in 0..10000 {
            std::hint::black_box(i);
        }
        assert!(span.elapsed().is_some_and(|n| n > Duration::ZERO));
    }
}
