use std::time::{Duration, Instant};
use tokio::time::sleep;

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

#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default(); // "", "spawn", "blocking"

    // heartbeat: 100 ms'de bir uyanmak İSTER; en kötü gecikmesini (Δ) ölçer
    let hb = tokio::spawn(async {
        let mut worst = Duration::ZERO;
        for _ in 0..50 {                       // 50 × 100 ms ≈ 5 s
            let planned = Instant::now() + Duration::from_millis(100);
            sleep(Duration::from_millis(100)).await;
            let delta = planned.elapsed();     // planlanandan ne kadar geç kaldım?
            if delta > worst { worst = delta; }
        }
        worst
    });

    match mode.as_str() {
        "spawn"    => { for _ in 0..2 { tokio::spawn(async { burn(5) }); } }
        "blocking" => { for _ in 0..2 { tokio::task::spawn_blocking(|| burn(5)); } }
        _          => {} // kontrol: burn yok
    }

    let worst = hb.await.unwrap();
    println!("mode={:8}  worst heartbeat delay: {:.1} ms",
             if mode.is_empty() { "control" } else { &mode },
             worst.as_secs_f64() * 1000.0);
}