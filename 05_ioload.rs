use std::time::{Duration, Instant};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let tasks: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000);
    println!("PID: {}", std::process::id());

    let start = Instant::now();
    let handles: Vec<_> = (0..tasks).map(|_| tokio::spawn(async {
        let mut worst = Duration::ZERO;
        for _ in 0..50 {                                  // her task = 50 turlu bir heartbeat
            let planned = Instant::now() + Duration::from_millis(100);
            sleep(Duration::from_millis(100)).await;
            let delta = planned.elapsed();
            if delta > worst { worst = delta; }
        }
        worst
    })).collect();

    let mut worst_all = Duration::ZERO;                   // TÜM task'ların en kötüsü
    for h in handles {
        let w = h.await.unwrap();
        if w > worst_all { worst_all = w; }
    }
    println!("{tasks} task(s): finished in {:.2} s, worst wake-up delay: {:.1} ms",
             start.elapsed().as_secs_f64(),
             worst_all.as_secs_f64() * 1000.0);
}