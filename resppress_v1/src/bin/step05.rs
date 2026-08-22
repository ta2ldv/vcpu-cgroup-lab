// ═══════════════════════════════════════════════════════════════════════
// RespPress — step05: latency ölçümü, percentile'lar, süslü rapor
// ═══════════════════════════════════════════════════════════════════════
// Ne yapar (step04'ün üstüne):
//   1. HER İSTEĞİN latency'sini ölçer: isteğin saati, içinde olduğu
//      BATCH'İN GÖNDERİM ANINDA başlar (batch'tekiler tele birlikte biner);
//      cevabı parse edildiği an durur → fark µs olarak lats'a yazılır.
//      Batch içindeki İLK cevaplar erken, SON cevaplar geç → batch'in
//      latency bedeli artık istek istek görünür.
//   2. Koşu sonunda lats sıralanır (sort_unstable) ve 5.3'ün tanımıyla
//      percentile okunur: pN = sıralı listede %N konumundaki değer.
//   3. Süslü rapor: box-drawing latency tablosu (unit/min/p50/p90/p99/
//      p99.9/max/avg) + bloklu histogram (█ + 1/8'lik uçlar ▏▎▍▌▋▊▉ →
//      %0.25 hassasiyet; 50 karakter = %100; boş bucket basılmaz).
//
// Ölçümler (t3.large, localhost, 200k istek) — BATCH↔LATENCY PAZARLIĞI:
//   batch=1    →   17k req/s   p50=55µs   p99=164   avg=58
//   batch=10   →  145k req/s   p50=61µs   p99=203   avg=68   ← bedava bölge!
//   batch=100  →  691k req/s   p50=113µs  p99=566   avg=141
//   batch=1000 → 1.47M req/s   p50=468µs  p99=962   avg=521
//   Ders 1: 1→10 arası throughput 8.5× artarken p50 yalnız +6µs — pipelining'in
//           altın aralığı (10 istek zaten ödenen RTT'yi paylaşır).
//   Ders 2: sonrası pazarlık: 100→1000'de hız 2× ama p50 4× — her istek bin
//           kişilik konvoyunun kuyruğunu bekler. "Batch throughput'u şişirir,
//           latency'yi öldürür" cümlesi tabloya dönüştü.
//   Ders 3: histogram her koşuda TEK TEPE (sağlıklı servis, 5.3) — çıplak
//           Redis'te kimse donmuyor. İki tepeli hali Part 7'de throttle'lı
//           pod'da avlanacak; baseline bu.
//
// Ölçüm maliyeti: cevap başına 1 Instant okuma (~20-40ns) + histogram
// güncellemesi (~ns'ler) — istek maliyetinin (~0.7µs+) yüzde birkaçı,
// gürültünün altında (step04 ile aynı platoda koştu).
//
// Koşu:  cargo run --release -- 200000 100
// ═══════════════════════════════════════════════════════════════════════

use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

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

fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    buf[from..].windows(2).position(|w| w == b"\r\n").map(|i| i + from)
}

fn parse_one(buf: &[u8]) -> Option<(bool, usize)> {
    match *buf.first()? {
        b'+' | b':' => {
            let end = find_crlf(buf, 1)?;
            Some((false, end + 2))
        }
        b'-' => {
            let end = find_crlf(buf, 1)?;
            Some((true, end + 2))
        }
        b'$' => {
            let hdr_end = find_crlf(buf, 1)?;
            let len: i64 = std::str::from_utf8(&buf[1..hdr_end]).ok()?.parse().ok()?;
            if len < 0 {
                return Some((false, hdr_end + 2));
            }
            let total = hdr_end + 2 + len as usize + 2;
            if buf.len() >= total { Some((false, total)) } else { None }
        }
        b'*' => {
            let hdr_end = find_crlf(buf, 1)?;
            let count: i64 = std::str::from_utf8(&buf[1..hdr_end]).ok()?.parse().ok()?;
            let mut pos = hdr_end + 2;
            let mut any_err = false;
            for _ in 0..count.max(0) {
                let (e, used) = parse_one(&buf[pos..])?;
                any_err |= e;
                pos += used;
            }
            Some((any_err, pos))
        }
        _ => Some((true, 1)),
    }
}

// Sıralı latency listesinden pN'i oku: pN = değerlerin %N'inin altında
// kaldığı değer (5.3'ün tanımı). Liste KÜÇÜKTEN BÜYÜĞE sıralı olmalı.
fn percentile(sorted: &[u32], p: f64) -> u32 {
    if sorted.is_empty() { return 0; }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx]
}

// Histogram bucket'ları: kuyruk detayı baş detayından kıymetli olduğu için
// sınırlar logaritmik büyür (5.3'ün dersi).
const BUCKETS: &[(u32, &str)] = &[
    (50,        "   <50µs "),
    (100,       " 50-100µs"),
    (200,       "100-200µs"),
    (500,       "200-500µs"),
    (1_000,     " 0.5-1ms "),
    (2_000,     "  1-2ms  "),
    (5_000,     "  2-5ms  "),
    (10_000,    " 5-10ms  "),
    (u32::MAX,  "  >10ms  "),
];

const BAR_WIDTH: usize = 50;    // çubuk alanı: 50 karakter = %100 (karakter başına %2)

// Yüzdeyi bloklu çubuğa çevir: tam kısım '█', kesir 1/8'lik uç bloklarla
// (▏▎▍▌▋▊▉) çizilir → %0.25 hassasiyet. Sıfır olmayan en küçük değer bile
// en az '▏' alır — "var ama az" görünür kalsın.
fn bar(pct: f64) -> String {
    let eighths = (pct / 100.0 * BAR_WIDTH as f64 * 8.0).round() as usize;
    let (full, rem) = (eighths / 8, eighths % 8);
    let mut s = "█".repeat(full);
    if rem > 0 {
        s.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][rem - 1]);
    }
    if s.is_empty() && pct > 0.0 {
        s.push('▏');
    }
    s
}

fn print_histogram(lats: &[u32]) {
    let mut counts = vec![0u64; BUCKETS.len()];
    for &l in lats {
        let i = BUCKETS.iter().position(|&(limit, _)| l < limit).unwrap();
        counts[i] += 1;
    }
    let total = lats.len() as f64;
    println!("histogram:");
    println!("┌───────────┬{}┬────────┐", "─".repeat(BAR_WIDTH + 2));
    for (i, &(_, label)) in BUCKETS.iter().enumerate() {
        if counts[i] == 0 { continue; }            // boş bucket'ı basma
        let pct = counts[i] as f64 / total * 100.0;
        println!("│ {label} │ {:<BAR_WIDTH$} │ {pct:>5.1}% │", bar(pct));
    }
    println!("└───────────┴{}┴────────┘", "─".repeat(BAR_WIDTH + 2));
}

// Latency özet tablosu — box-drawing çerçeveli.
fn print_latency_table(sorted: &[u32], avg: f64) {
    println!("latency:");
    println!("┌──────┬───────┬───────┬───────┬───────┬────────┬────────┬───────┐");
    println!("│ unit │   min │   p50 │   p90 │   p99 │  p99.9 │    max │   avg │");
    println!("├──────┼───────┼───────┼───────┼───────┼────────┼────────┼───────┤");
    println!("│   µs │ {:>5} │ {:>5} │ {:>5} │ {:>5} │ {:>6} │ {:>6} │ {:>5.0} │",
             sorted.first().copied().unwrap_or(0),
             percentile(sorted, 50.0),
             percentile(sorted, 90.0),
             percentile(sorted, 99.0),
             percentile(sorted, 99.9),
             sorted.last().copied().unwrap_or(0),
             avg);
    println!("└──────┴───────┴───────┴───────┴───────┴────────┴────────┴───────┘");
}

#[tokio::main]
async fn main() {
    let n: usize          = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let batch_size: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(1);
    println!("PID: {}", std::process::id());
    println!("Sending {n} messages (SET/GET mix 50/50, batch size {batch_size}) to 127.0.0.1:6379 ...");

    let mut stream = TcpStream::connect("127.0.0.1:6379").await.unwrap();
    let mut buf = [0u8; 65536];

    let set = encode(&["SET", "foo", "bar"]);
    let get = encode(&["GET", "foo"]);
    let mut batch = Vec::new();
    for i in 0..batch_size {
        batch.extend_from_slice(if i % 2 == 0 { &set } else { &get });
    }

    let rounds = n / batch_size;
    let mut oks: u64 = 0;
    let mut errors: u64 = 0;
    let mut first_error: Option<String> = None;
    let mut acc: Vec<u8> = Vec::new();

    // LATENCY KAYDI: her cevabın gecikmesi µs olarak buraya birikir.
    // Bir isteğin saati, İÇİNDE OLDUĞU BATCH'İN GÖNDERİM ANINDA başlar
    // (batch'tekiler tele birlikte biner; isteğin "yaşadığı süre" budur).
    // Cevap parse edildiği an saat durur → aradaki fark = o isteğin latency'si.
    // Batch büyüdükçe son cevapların gecikmesi büyür — "batch throughput'u
    // şişirir, latency'yi öldürür" dersi artık ÖLÇÜLEBİLİR.
    let mut lats: Vec<u32> = Vec::with_capacity(n);

    let start = Instant::now();
    for round in 0..rounds {
        let batch_sent = Instant::now();           // bu batch'in saat başlangıcı
        stream.write_all(&batch).await.unwrap();

        let mut replies = 0;
        let mut pos = 0;
        while replies < batch_size {
            while replies < batch_size {
                match parse_one(&acc[pos..]) {
                    Some((is_err, used)) => {
                        // cevap TAMAMLANDI → bu isteğin latency'si belli oldu
                        lats.push(batch_sent.elapsed().as_micros() as u32);
                        if is_err {
                            errors += 1;
                            if first_error.is_none() {
                                first_error = Some(
                                    String::from_utf8_lossy(&acc[pos..(pos + used).min(pos + 64)])
                                        .trim_end().to_string());
                            }
                        } else {
                            oks += 1;
                        }
                        pos += used;
                        replies += 1;
                    }
                    None => break,
                }
            }
            if replies == batch_size { break; }

            match timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
                Err(_) => {
                    eprintln!("ERROR: read timeout (round {round}, {replies}/{batch_size} replies)");
                    std::process::exit(1);
                }
                Ok(Ok(0)) => {
                    eprintln!("ERROR: connection closed by server (round {round}, {replies}/{batch_size} replies)");
                    std::process::exit(1);
                }
                Ok(Ok(m)) => acc.extend_from_slice(&buf[..m]),
                Ok(Err(e)) => {
                    eprintln!("ERROR: read failed (round {round}): {e}");
                    std::process::exit(1);
                }
            }
        }
        acc.drain(..pos);
    }
    let wall = start.elapsed().as_secs_f64();

    let total = rounds * batch_size;
    println!("{total} requests in {wall:.2} s  ->  {:.0} req/s",
             total as f64 / wall);
    println!("replies: {oks} ok, {errors} errors{}",
             match &first_error {
                 Some(e) => format!("  (first error: {e:?})"),
                 None => String::new(),
             });

    // ── LATENCY RAPORU ──────────────────────────────────────────────────
    // Percentile için tek gereken: listeyi sırala, konumdan oku (5.3).
    lats.sort_unstable();
    let avg = lats.iter().map(|&l| l as u64).sum::<u64>() as f64 / lats.len() as f64;
    print_latency_table(&lats, avg);
    print_histogram(&lats);
}
