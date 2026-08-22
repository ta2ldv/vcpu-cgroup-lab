// ═══════════════════════════════════════════════════════════════════════
// RespPress — step03: batch gönderimi + iki bekçi (fail-fast)
// ═══════════════════════════════════════════════════════════════════════
// Ne yapar:
//   1. batch_size kadar SET frame'ini TEK buffer'a yapıştırır
//      (5.2'deki printf|nc numarasının programlısı)
//   2. Her turda: batch'i tek write'ta gönderir, cevapların TAMAMI
//      (batch_size × 5 byte) gelene kadar okur — cevaplar sıralı ve
//      hepsi "+OK\r\n" olduğu için BYTE SAYMAK = CEVAP SAYMAKTIR
//   3. Sonunda toplam istek, süre, req/s, avg µs basar
//
// step02'den farklar:
//   - 2. argüman: batch_size (1 = eski ping-pong davranışı, kontrol grubu)
//   - 64 KB okuma buffer'ı (toplu cevaplar için)
//   - FIX 1: read()==0 → server kapattı → ERROR + exit (eskiden sonsuz döngüydü!)
//   - FIX 2: içerik bekçisi — akışta '+','O','K','\r','\n' dışında byte
//     görülürse (örn. "-ERR ...") ERROR + exit. OK-only dünyaya özel kestirme;
//     gerçek parser step04'te gelir.
//
// Politika: fail-fast — ilk anomalide kes. (Tek hata bile byte-sayma
// muhasebesini bozar; bozuk deney sürdürülmez. step04'te parser gelince
// politika "say ve devam et"e döner.)
//
// Ölçümler (t3.large, localhost, 50k istek):
//   batch=1: 16.1k · 10: 133k · 100: 682k · 1000: 855k
//   10k: 1.37M · 50k: 1.43M req/s  (77×; taban 0.70 µs/req = işleme maliyeti)
// Uzun koşular (5M istek): plato ~1.35-1.4M kararlı.
// Şahitlik: CONFIG RESETSTAT → 4×5M koşu → total_commands_processed
//   = 20.000.001 (20M SET + 1 INFO) — kuruş kuruş doğrulandı.
// Kopuş testi: koşu ortasında SHUTDOWN NOSAVE →
//   "ERROR: connection closed by server (round 3538, got 2640/5000 bytes)"
//
// Ders: batch throughput'u şişirir ama LATENCY kavramını öldürür — bir
// isteğin cevabı koca turun sonunda gelir. Ölçüm için ılımlı batch (10-100),
// tavan gösterisi için 1000+. Terminoloji: bu "pipeline depth" DEĞİL —
// kayan pencere yok; paket-gönder-hepsini-bekle usulü = batch.
//
// Koşu:  cargo run --release -- 5000000 1000
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
    let n: usize          = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let batch_size: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(1);
    println!("PID: {}", std::process::id());
    println!("Sending {n} messages (SET foo bar, batch size {batch_size}) to 127.0.0.1:6379 ...");

    let mut stream = TcpStream::connect("127.0.0.1:6379").await.unwrap();
    let mut buf = [0u8; 65536];

    let frame = encode(&["SET", "foo", "bar"]);

    let mut batch = Vec::new();
    for _ in 0..batch_size {
        batch.extend_from_slice(&frame);
    }

    let rounds = n / batch_size;
    let expected = batch_size * 5;                // her SET cevabı "+OK\r\n" = 5 byte

    let start = Instant::now();
    for round in 0..rounds {
        stream.write_all(&batch).await.unwrap();
        let mut got = 0;
        while got < expected {
            let m = stream.read(&mut buf).await.unwrap();

            if m == 0 {                           // FIX 1: kopuş tespiti
                eprintln!("ERROR: connection closed by server (round {round}, got {got}/{expected} bytes)");
                std::process::exit(1);
            }

            for &b in &buf[..m] {                 // FIX 2: içerik bekçisi
                if !matches!(b, b'+' | b'O' | b'K' | b'\r' | b'\n') {
                    eprintln!("ERROR: unexpected reply byte {:?} (round {round}); reply starts: {:?}",
                              b as char, String::from_utf8_lossy(&buf[..m.min(64)]));
                    std::process::exit(1);
                }
            }

            got += m;
        }
    }
    let wall = start.elapsed().as_secs_f64();

    let total = rounds * batch_size;
    println!("{total} requests in {wall:.2} s  ->  {:.0} req/s  (avg {:.2} µs/req)",
             total as f64 / wall,
             wall / total as f64 * 1_000_000.0);
}
