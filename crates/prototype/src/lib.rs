//! Platform-neutral Stage 2 state and fixed-height conversation virtualization.
#![forbid(unsafe_code)]
use std::{
    collections::VecDeque,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

pub const SESSION_COUNT: usize = 20;
pub const EVENT_COUNT: usize = 100_000;
pub const ROW_HEIGHT: f64 = 72.0;
pub const SAMPLE_LIMIT: usize = 4096;

#[derive(Debug)]
pub struct Record {
    pub role: &'static str,
    pub text: &'static str,
}

#[derive(Debug)]
pub struct Session {
    pub id: usize,
    pub name: String,
    pub records: Arc<[Record]>,
    pub scroll_y: f64,
    pub draft: String,
    pub settings_open: bool,
    pub model: usize,
    pub updates: u64,
    pub live_text: String,
}
impl Session {
    pub fn row_count(&self) -> usize {
        self.records.len() + 1
    }
    pub fn content_height(&self) -> f64 {
        self.row_count() as f64 * ROW_HEIGHT
    }
}

pub struct Prototype {
    pub sessions: Vec<Session>,
    active: usize,
}
impl Default for Prototype {
    fn default() -> Self {
        Self::new()
    }
}
impl Prototype {
    pub fn new() -> Self {
        let records: Arc<[Record]> = (0..EVENT_COUNT).map(|i| Record {
            role: if i % 3 == 0 { "Developer" } else { "Mock agent" },
            text: match i % 3 { 0 => "Investigate this development task and explain the next step.",
                1 => "Inspecting the workspace. This is synthetic UI data, not a Copilot response.",
                _ => "Proposed change ready for review. No files or commands were executed." },
        }).collect();
        let roles = ["Builder", "Reviewer", "Researcher", "Tester", "Planner"];
        Self {
            sessions: (0..SESSION_COUNT)
                .map(|id| Session {
                    id,
                    name: format!("{} {:02}", roles[id % roles.len()], id + 1),
                    records: records.clone(),
                    scroll_y: 0.0,
                    draft: String::new(),
                    settings_open: false,
                    model: 0,
                    updates: 0,
                    live_text: String::with_capacity(1024),
                })
                .collect(),
            active: 0,
        }
    }
    pub fn active_index(&self) -> usize {
        self.active
    }
    pub fn active(&self) -> &Session {
        &self.sessions[self.active]
    }
    pub fn active_mut(&mut self) -> &mut Session {
        &mut self.sessions[self.active]
    }
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.sessions.len() {
            return false;
        }
        self.active = index;
        true
    }
    pub fn remember_scroll(&mut self, y: f64, height: f64) {
        self.active_mut().scroll_y = clamp_scroll(y, height, self.active().content_height());
    }
    pub fn apply(&mut self, update: &Update) {
        if let Some(session) = self.sessions.get_mut(update.session) {
            session.updates += 1;
            if session.live_text.len() + update.text.len() > 1024 {
                session.live_text.clear();
            }
            session.live_text.push_str(update.text);
        }
    }
}
fn nonnegative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
pub fn clamp_scroll(y: f64, viewport: f64, content: f64) -> f64 {
    nonnegative(y).min((nonnegative(content) - nonnegative(viewport)).max(0.0))
}
/// Fixed-height cached layout; returns visible rows and two rows of overscan.
/// Its cost and returned range depend on viewport height, not history length.
pub fn visible_rows(y: f64, height: f64, count: usize) -> Range<usize> {
    if count == 0 || !height.is_finite() || height <= 0.0 {
        return 0..0;
    }
    let y = clamp_scroll(y, height, count as f64 * ROW_HEIGHT);
    let first = (y / ROW_HEIGHT).floor() as usize;
    let end = ((y + height) / ROW_HEIGHT).ceil() as usize;
    first.saturating_sub(2)..end.saturating_add(2).min(count)
}

pub struct Update {
    pub session: usize,
    pub text: &'static str,
    pub received: Instant,
}
/// One bounded producer. Mock updates can be dropped under saturation; real SDK
/// integration will need explicit resynchronization rather than this mock policy.
pub struct MockWorker {
    rx: Receiver<Update>,
    stop: Arc<AtomicBool>,
}
impl Default for MockWorker {
    fn default() -> Self {
        Self::new()
    }
}
impl MockWorker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(64);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        thread::spawn(move || {
            let mut tick = 0;
            while !worker_stop.load(Ordering::Relaxed) {
                let update = Update {
                    session: tick % SESSION_COUNT,
                    text: "Mock streamed activity. ",
                    received: Instant::now(),
                };
                match tx.try_send(update) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
                tick += 1;
                thread::sleep(Duration::from_millis(10));
            }
        });
        Self { rx, stop }
    }
    /// Main-thread work is bounded even if the producer is busy.
    pub fn drain(&self) -> impl Iterator<Item = Update> + '_ {
        self.rx.try_iter().take(32)
    }
}
impl Drop for MockWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Samples {
    values: VecDeque<u64>,
}
#[derive(Debug, Clone, Copy)]
pub struct Percentiles {
    pub count: usize,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
}
impl Samples {
    pub fn record(&mut self, elapsed: Duration) {
        if self.values.len() == SAMPLE_LIMIT {
            self.values.pop_front();
        }
        self.values
            .push_back(elapsed.as_nanos().min(u64::MAX as u128) as u64);
    }
    pub fn percentiles(&self) -> Option<Percentiles> {
        if self.values.is_empty() {
            return None;
        }
        let mut sorted: Vec<_> = self.values.iter().copied().collect();
        sorted.sort_unstable();
        let percentile = |p: usize| sorted[(sorted.len() * p).div_ceil(100).saturating_sub(1)];
        Some(Percentiles {
            count: sorted.len(),
            p50: percentile(50),
            p95: percentile(95),
            p99: percentile(99),
        })
    }
}
#[derive(Default)]
pub struct Metrics {
    pub switching: Samples,
    pub switch_to_draw: Samples,
    pub settings: Samples,
    pub drawing: Samples,
    pub event_to_draw: Samples,
    pub rows_drawn: usize,
    pub max_rows_drawn: usize,
}
impl Metrics {
    pub fn report(&self) -> String {
        let mut report = String::from(
            "Timing units: nanoseconds. CPU callbacks are not presentation latency.\n",
        );
        for (name, samples) in [
            ("session_switch_cpu", &self.switching),
            ("session_switch_to_draw_callback", &self.switch_to_draw),
            ("settings_cpu", &self.settings),
            ("draw_cpu", &self.drawing),
            ("mock_event_to_draw_callback", &self.event_to_draw),
        ] {
            if let Some(p) = samples.percentiles() {
                report.push_str(&format!(
                    "{name}: n={} p50={} p95={} p99={}\n",
                    p.count, p.p50, p.p95, p.p99
                ));
            }
        }
        report.push_str(&format!(
            "rows_last_draw={} max_rows_per_draw={}\n",
            self.rows_drawn, self.max_rows_drawn
        ));
        report
    }
}

/// Headless algorithm benchmark. Never a substitute for native display measurements.
pub fn benchmark() -> String {
    let setup = Instant::now();
    let mut state = Prototype::new();
    let setup = setup.elapsed();
    let mut switches = Samples::default();
    let mut scrolling = Samples::default();
    let mut max_rows = 0;
    for i in 0..20_000 {
        let start = Instant::now();
        state.select(std::hint::black_box(i % SESSION_COUNT));
        std::hint::black_box(state.active());
        switches.record(start.elapsed());
        let start = Instant::now();
        let y = (i * 7919 % EVENT_COUNT) as f64 * ROW_HEIGHT;
        let range = visible_rows(std::hint::black_box(y), 800.0, state.active().row_count());
        max_rows = max_rows.max(range.len());
        std::hint::black_box(range);
        scrolling.record(start.elapsed());
    }
    format!(
        "Headless algorithms only; host={}-{}; 20 sessions, 100000 shared immutable mock records.\nsetup={setup:?}\nswitch_cpu_ns={:?}\nvisible_range_cpu_ns={:?}\nmax_visible_rows={max_rows}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        switches.percentiles(),
        scrolling.percentiles()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_navigation_preserves_owned_state_and_shared_history() {
        let mut p = Prototype::new();
        p.active_mut().draft = "日本語 🦀 draft".into();
        p.active_mut().settings_open = true;
        p.active_mut().model = 1;
        p.remember_scroll(7200.0, 800.0);
        let history = p.active().records.clone();
        for i in 1..SESSION_COUNT {
            assert!(p.select(i));
            assert!(p.active().draft.is_empty());
        }
        p.select(0);
        assert_eq!(p.active().scroll_y, 7200.0);
        assert_eq!(p.active().draft, "日本語 🦀 draft");
        assert!(p.active().settings_open);
        assert_eq!(p.active().model, 1);
        assert!(Arc::ptr_eq(&history, &p.active().records));
        assert!(!p.select(SESSION_COUNT));
        assert_eq!(p.active_index(), 0);
    }
    #[test]
    fn virtual_ranges_cover_viewport_without_scaling_with_history() {
        for count in [1, 100_000, 1_000_000] {
            for y in [0.0, 71.0, 10_000.0, f64::MAX, f64::NAN, -10.0] {
                let range = visible_rows(y, 800.0, count);
                assert!(range.end <= count);
                assert!(range.len() <= 17);
                let clamped = clamp_scroll(y, 800.0, count as f64 * ROW_HEIGHT);
                assert!(range.contains(&((clamped / ROW_HEIGHT).floor() as usize)));
            }
        }
        assert_eq!(visible_rows(0.0, 0.0, 100), 0..0);
        assert_eq!(visible_rows(0.0, 800.0, 0), 0..0);
    }
    #[test]
    fn background_updates_do_not_move_scroll_or_change_other_sessions() {
        let mut p = Prototype::new();
        p.remember_scroll(4000.0, 800.0);
        let u = Update {
            session: 0,
            text: "🦀",
            received: Instant::now(),
        };
        for _ in 0..2000 {
            p.apply(&u);
        }
        assert_eq!(p.active().scroll_y, 4000.0);
        assert_eq!(p.active().updates, 2000);
        assert!(p.active().live_text.len() <= 1024);
        assert_eq!(p.sessions[1].updates, 0);
    }
    #[test]
    fn telemetry_is_bounded_and_uses_nearest_rank_percentiles() {
        let mut s = Samples::default();
        assert!(s.percentiles().is_none());
        for n in 1..=100 {
            s.record(Duration::from_nanos(n));
        }
        let p = s.percentiles().unwrap();
        assert_eq!((p.p50, p.p95, p.p99), (50, 95, 99));
        for _ in 0..SAMPLE_LIMIT {
            s.record(Duration::from_nanos(7));
        }
        let p = s.percentiles().unwrap();
        assert_eq!(p.count, SAMPLE_LIMIT);
        assert_eq!(p.p99, 7);
    }
}
