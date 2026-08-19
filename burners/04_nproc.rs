use std::thread;

fn main() {
    match thread::available_parallelism() {
        Ok(n) => println!("available_parallelism: {n}"),
        Err(e) => println!("error: {e}"),
    }
}
