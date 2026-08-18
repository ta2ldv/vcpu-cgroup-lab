use std::time::{Duration, Instant};

fn main() {
    let secs = 5;
    let start = Instant::now();
    let mut count: u64 = 0;

    while start.elapsed() < Duration::from_secs(secs) {
        count += 1;
    }

    let rate = count as f64 / secs as f64 / 1_000_000.0;
    println!("{count} iterations in {secs} s  ->  {rate:.1} M iter/s");
}
