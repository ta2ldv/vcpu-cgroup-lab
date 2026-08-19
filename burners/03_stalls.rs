use std::env;
use std::time::{Duration, Instant};

const BATCH: u64 = 1_000_000;

// One thread's work: count for `secs` seconds, and this time also record
// the LONGEST gap between two batch completions — the "max stall".
// A batch normally takes a few ms; if the kernel freezes us (quota exhausted),
// the next batch completes tens of ms late, and the gap betrays the freeze.
fn burn(secs: u64) -> (u64, Duration) {
    let start = Instant::now();
    let mut count: u64 = 0;
    let mut max_stall = Duration::ZERO;
    let mut last = Instant::now();

    while start.elapsed() < Duration::from_secs(secs) {
        for _ in 0..BATCH {
            count = std::hint::black_box(count + 1);
        }
        let now = Instant::now();
        let gap = now - last;          // time this batch took, freezes included
        if gap > max_stall {
            max_stall = gap;
        }
        last = now;
    }
    (count, max_stall)
}

fn main() {
    let secs = 5;
    let threads: usize = env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);

    let start = Instant::now();
    let handles: Vec<_> = (0..threads).map(|_| std::thread::spawn(move || burn(secs))).collect();

    let mut total: u64 = 0;
    let mut worst_stall = Duration::ZERO;   // the worst stall seen by ANY thread
    for h in handles {
        let (count, max_stall) = h.join().unwrap();
        total += count;
        if max_stall > worst_stall {
            worst_stall = max_stall;
        }
    }
    let wall = start.elapsed().as_secs_f64();

    let rate = total as f64 / wall / 1_000_000.0;
    println!(
        "{threads} thread(s): {rate:.0} M iter/s total, worst stall: {:.1} ms",
        worst_stall.as_secs_f64() * 1000.0
    );
}
