use std::env;
use std::time::{Duration, Instant};

const BATCH: u64 = 1_000_000;

fn burn(secs: u64) -> u64 {
    let start = Instant::now();
    let mut count: u64 = 0;
    while start.elapsed() < Duration::from_secs(secs) {
        for _ in 0..BATCH {
            count = std::hint::black_box(count + 1);
        }
    }
    count
}

fn main() {
    let secs = 5;
    let threads: usize = env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);

    let start = Instant::now();
    let handles: Vec<_> = (0..threads).map(|_| std::thread::spawn(move || burn(secs))).collect();
    let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    let wall = start.elapsed().as_secs_f64();

    let rate = total as f64 / wall / 1_000_000.0;
    println!("{threads} thread(s): {total} iters in {wall:.2} s  ->  {rate:.0} M iter/s total");
}
