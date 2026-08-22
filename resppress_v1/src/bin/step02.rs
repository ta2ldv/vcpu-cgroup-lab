// ═══════════════════════════════════════════════════════════════════════
// RespPress — step02: 10k+ mesajlık ping-pong (ilk ölçüm)
// ═══════════════════════════════════════════════════════════════════════
// Ne yapar:
//   1. PID + "Sending N messages" banner'ı basar (koşu kimliği başta)
//   2. Tek bağlantıdan N kez SET foo bar gönderir — HER İSTEKTEN SONRA
//      CEVABI BEKLEYEREK (= ping-pong: vur, karşılığını bekle, yine vur)
//   3. Süreyi tutar; req/s ve istek başına ortalama µs basar
//
// step01'den farklar:
//   - argümandan N (default 10_000)
//   - frame döngü DIŞINDA bir kez kurulur (her turda encode çağırmak
//     client CPU'sunu ölçmek olurdu; biz teli ölçüyoruz)
//   - kronometre + türetilmiş sayılar
//
// Ölçtüğü şey: ping-pong'un tavanı ≈ 1/RTT. Süreyi mesafe (gidiş-dönüş
// beklemesi) belirler, server değil — Redis bu koşuda neredeyse yatar.
// Gerçek ölçüm (t3.large, localhost): 50000 istek, 2.68 s,
// 18630 req/s, avg 53.7 µs/req.
//
// Koşu:  cargo run --release -- 50000
// ═══════════════════════════════════════════════════════════════════════

use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn encode(cmd: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("*{}\r\n", cmd.len()).as_bytes());
    for part in cmd {
        out.extend_from_slice(format!("${}\r\n", part.len()).as_bytes());
        out.extend_from_slice(part.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[tokio::main]
async fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    println!("PID: {}", std::process::id());
    println!("Sending {n} messages (SET foo bar, ping-pong) to 127.0.0.1:6379 ...");

    let mut stream = TcpStream::connect("127.0.0.1:6379").await.unwrap();
    let mut buf = [0u8; 4096];

    let frame = encode(&["SET", "foo", "bar"]);   // frame'i BİR KEZ kur, N kez gönder

    let start = Instant::now();
    for _ in 0..n {
        stream.write_all(&frame).await.unwrap();  // vur   (istek gitti)
        stream.read(&mut buf).await.unwrap();     // BEKLE (cevap gelene kadar dur) — ping-pong
    }
    let wall = start.elapsed().as_secs_f64();

    println!("{n} requests in {wall:.2} s  ->  {:.0} req/s  (avg {:.1} µs/req)",
             n as f64 / wall,
             wall / n as f64 * 1_000_000.0);
}
