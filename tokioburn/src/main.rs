use std::thread;
use std::time::Duration;

fn main() {
    println!("available_parallelism: {:?}", thread::available_parallelism());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    println!("tokio workers: {}  (PID: {})", rt.metrics().num_workers(), std::process::id());

    rt.block_on(async {
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
}