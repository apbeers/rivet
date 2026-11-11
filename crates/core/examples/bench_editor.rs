//! Shared-buffer CPU measurements; never a native UI latency benchmark.
use rivet_core::editor::Buffer;
use std::{sync::Arc, time::Instant};
fn report(name: &str, mut samples: Vec<u128>) {
    samples.sort_unstable();
    let p = |n: usize| samples[(samples.len() * n).div_ceil(100).saturating_sub(1)];
    println!(
        "{name}: n={} p50={}us p95={}us p99={}us",
        samples.len(),
        p(50),
        p(95),
        p(99)
    );
}
fn main() {
    println!(
        "Shared editor CPU only; host={}-{}; no native layout, frames, or filesystem latency",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let text: Arc<str> = Arc::from("fn example() { answer(); }\n".repeat(10 * 1024 * 1024 / 26));
    let start = Instant::now();
    let mut buffer = Buffer::new(text);
    println!(
        "buffer_10mb_create={}us bytes={} lines={}",
        start.elapsed().as_micros(),
        buffer.len(),
        buffer.line_count()
    );
    let mut samples = Vec::new();
    for _ in 0..100 {
        let start = Instant::now();
        buffer.replace(0..0, "// edit 🦀\n").unwrap();
        samples.push(start.elapsed().as_micros());
        buffer.undo();
    }
    report("buffer_10mb_insert_first_line", samples);
    let mut long = Buffer::new(Arc::from("x".repeat(10 * 1024 * 1024)));
    let mut samples = Vec::new();
    for _ in 0..100 {
        let start = Instant::now();
        let len = long.len();
        long.replace(len..len, "🦀").unwrap();
        samples.push(start.elapsed().as_micros());
        long.undo();
    }
    report("buffer_10mb_single_line_append", samples);
}
