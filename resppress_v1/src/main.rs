use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

// ── Konfigürasyon: CLI flag'leri + hangilerinin default kaldığı ─────────
// Rapor, kullanıcının hangi düğmeye BİLİNÇLİ dokunduğunu da belgeler:
// default kalan her satır summary'de "(default)" işareti taşır.
struct Config {
    n: usize,            // -n  toplam istek
    batch: usize,        // -b  batch size
    rate: u64,           // -r  rate limit (0 = unlimited)
    host: String,        // -t  hedef host
    port: String,        // -t  hedef port (adreste ':' ile; yoksa 6379)
    n_def: bool,
    batch_def: bool,
    rate_def: bool,
    target_def: bool,    // -t hiç verilmedi
    port_def: bool,      // port belirtilmedi (default 6379 kullanıldı)
}

const DEF_N: usize = 10_000;
const DEF_BATCH: usize = 1;
const DEF_RATE: u64 = 0;
const DEF_TARGET: &str = "127.0.0.1:6379";

fn usage() {
    eprintln!("usage: resppress_v1 [-n N] [-b B] [-r R] [-t HOST:PORT]");
    eprintln!("  -n, --number      total requests       (default {DEF_N})");
    eprintln!("  -b, --batch-size  batch size           (default {DEF_BATCH} = ping-pong)");
    eprintln!("  -r, --rate        rate limit, req/s    (default {DEF_RATE} = unlimited)");
    eprintln!("  -t, --target      target address       (default {DEF_TARGET};");
    eprintln!("                    port omitted -> :6379 appended)");
}

// Elle flag parser — sihir yok, crate yok. Kurallar:
//   - her flag bir değer ister (--n 200000)
//   - bilinmeyen flag / eksik / bozuk değer = ERROR + usage + çıkış
//     (sessiz yutma yok: bekçi geleneği CLI'da da geçerli)
fn parse_args() -> Config {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut n, mut batch, mut rate, mut target) = (None, None, None, None);

    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "--help" || flag == "-h" {
            usage();
            std::process::exit(0);
        }
        let val = match args.get(i + 1) {
            Some(v) => v,
            None => {
                eprintln!("ERROR: flag '{flag}' needs a value");
                usage();
                std::process::exit(2);
            }
        };
        // DUPLICATE GUARD: aynı düğme iki kez verilirse (kısa/uzun karışık
        // olsa bile: -n 1000 --number 2000) sessizce ezmek yerine ERROR —
        // typo'yla yanlış deney koşulmasın.
        let dup = |name: &str| {
            eprintln!("ERROR: duplicate flag: {name} was already given (conflicting values)");
            std::process::exit(2);
        };
        match flag {
            "-n" | "--number" => {
                if n.is_some() { dup("-n/--number"); }
                n = Some(val.parse().unwrap_or_else(|_| {
                    eprintln!("ERROR: {flag} needs a number, got {val:?}");
                    std::process::exit(2);
                }));
            }
            "-b" | "--batch-size" => {
                if batch.is_some() { dup("-b/--batch-size"); }
                batch = Some(val.parse().unwrap_or_else(|_| {
                    eprintln!("ERROR: {flag} needs a number, got {val:?}");
                    std::process::exit(2);
                }));
            }
            "-r" | "--rate" => {
                if rate.is_some() { dup("-r/--rate"); }
                rate = Some(val.parse().unwrap_or_else(|_| {
                    eprintln!("ERROR: {flag} needs a number, got {val:?}");
                    std::process::exit(2);
                }));
            }
            "-t" | "--target" => {
                if target.is_some() { dup("-t/--target"); }
                target = Some(val.clone());
            }
            _ => {
                eprintln!("ERROR: unknown flag {flag:?}");
                usage();
                std::process::exit(2);
            }
        }
        i += 2;
    }

    // TARGET NORMALİZASYONU: "host:port" ':' üstünden ayrıştırılır;
    // ':' yoksa yalnız host verilmiştir → default RESP portu (6379).
    // "127.0.0.1" ile "127.0.0.1:3423" ayrımı böyle yapılır.
    let target_def = target.is_none();
    let t = target.unwrap_or_else(|| DEF_TARGET.to_string());
    let (host, port, port_def) = match t.split_once(':') {
        Some((h, p)) => (h.to_string(), p.to_string(), false),
        None => (t, "6379".to_string(), true),
    };

    Config {
        n_def: n.is_none(),
        batch_def: batch.is_none(),
        rate_def: rate.is_none(),
        target_def,
        port_def,
        n: n.unwrap_or(DEF_N),
        batch: batch.unwrap_or(DEF_BATCH),
        rate: rate.unwrap_or(DEF_RATE),
        host,
        port,
    }
}

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

// Latency özet tablosu — box-drawing çerçeveli, DİNAMİK kolon genişliği:
// her kolon, başlığı ile değerinden hangisi uzunsa ona göre genişler —
// 6-7 haneli µs değerleri (CO koşuları) tabloyu asla taşırmaz.
fn print_latency_table(sorted: &[u32], avg: f64) {
    let headers = ["unit", "min", "p50", "p90", "p99", "p99.9", "max", "avg"];
    let values = [
        "µs".to_string(),
        sorted.first().copied().unwrap_or(0).to_string(),
        percentile(sorted, 50.0).to_string(),
        percentile(sorted, 90.0).to_string(),
        percentile(sorted, 99.0).to_string(),
        percentile(sorted, 99.9).to_string(),
        sorted.last().copied().unwrap_or(0).to_string(),
        format!("{avg:.0}"),
    ];
    let widths: Vec<usize> = headers.iter().zip(&values)
        .map(|(h, v)| h.chars().count().max(v.chars().count()))
        .collect();

    let border = |l: &str, m: &str, r: &str| {
        let mid = widths.iter().map(|w| "─".repeat(w + 2)).collect::<Vec<_>>().join(m);
        println!("{l}{mid}{r}");
    };
    let row = |cells: Vec<String>| {
        let mid = cells.iter().zip(&widths)
            .map(|(c, w)| format!(" {c:>w$} "))
            .collect::<Vec<_>>().join("│");
        println!("│{mid}│");
    };

    println!("latency:");
    border("┌", "┬", "┐");
    row(headers.iter().map(|s| s.to_string()).collect());
    border("├", "┼", "┤");
    row(values.to_vec());
    border("└", "┴", "┘");
}

// Koşu özeti — config echo + sonuçlar tek tabloda. Default kalan her
// düğme "(default)" işareti taşır: rapor, neyin bilinçli seçildiğini söyler.
fn print_summary(cfg: &Config, wall: f64, total: usize, oks: u64, errors: u64) {
    let def = |d: bool| if d { " (default)" } else { "" };
    let rows: Vec<(&str, String)> = vec![
        ("target",     format!("{}{}", cfg.host, def(cfg.target_def))),
        ("port",       format!("{}{}", cfg.port, def(cfg.target_def || cfg.port_def))),
        ("workload",   "SET/GET mix 50/50".to_string()),
        ("requests",   format!("{}{}", cfg.n, def(cfg.n_def))),
        ("batch size", format!("{}{}", cfg.batch, def(cfg.batch_def))),
        ("rate",       if cfg.rate == 0 {
                           format!("unlimited{}", def(cfg.rate_def))
                       } else {
                           format!("{} req/s{}", cfg.rate, def(cfg.rate_def))
                       }),
        ("duration",   format!("{wall:.2} s")),
        ("achieved",   format!("{:.0} req/s", total as f64 / wall)),
        ("replies",    format!("{oks} ok, {errors} errors")),
    ];
    // değer sütunu, en uzun değere göre genişler (chars ile: µ tek sayılır)
    let w = rows.iter().map(|(_, v)| v.chars().count()).max().unwrap_or(0);
    println!("summary:");
    println!("┌────────────┬─{}─┐", "─".repeat(w));
    for (label, value) in &rows {
        println!("│ {label:<10} │ {value:<w$} │");
    }
    println!("└────────────┴─{}─┘", "─".repeat(w));
}

#[tokio::main]
async fn main() {
    let cfg = parse_args();
    let addr = format!("{}:{}", cfg.host, cfg.port);
    println!("PID: {}", std::process::id());
    println!("Sending {} messages to {addr} ...", cfg.n);

    let mut stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: cannot connect to {addr}: {e}");
            std::process::exit(1);
        }
    };
    let mut buf = [0u8; 65536];

    let set = encode(&["SET", "foo", "bar"]);
    let get = encode(&["GET", "foo"]);
    let mut batch = Vec::new();
    for i in 0..cfg.batch {
        batch.extend_from_slice(if i % 2 == 0 { &set } else { &get });
    }

    // SABİT RATE TAKVİMİ: rate>0 ise batch'ler takvimle gönderilir.
    // Batch aralığı = batch / rate saniye; i'inci batch'in planlanan anı
    // = start + i × aralık. rate=0 → takvim yok, yardır (tavan modu).
    let interval = if cfg.rate > 0 {
        Some(Duration::from_secs_f64(cfg.batch as f64 / cfg.rate as f64))
    } else {
        None
    };

    let rounds = cfg.n / cfg.batch;
    let mut oks: u64 = 0;
    let mut errors: u64 = 0;
    let mut first_error: Option<String> = None;
    let mut acc: Vec<u8> = Vec::new();

    // LATENCY KAYDI: her cevabın gecikmesi µs olarak buraya birikir.
    // İsteğin saati batch'inin (planlanan ya da gerçek) gönderim anında
    // başlar; cevabı parse edildiği an durur.
    let mut lats: Vec<u32> = Vec::with_capacity(cfg.n);

    let start = Instant::now();
    for round in 0..rounds {
        // COORDINATED OMISSION İLKESİ: sabit rate modunda saat, GERÇEK
        // gönderim anından değil PLANLANAN anından başlar. Server bizi
        // geriletirse o gecikme latency'ye YAZILIR (heartbeat'in
        // planned.elapsed() ilkesi). Geç kaldıysak uyumayız ama saat
        // plandan işler.
        let batch_sent = match interval {
            Some(iv) => {
                let sched = start + iv * round as u32;
                let now = Instant::now();
                if sched > now {
                    tokio::time::sleep(sched - now).await;
                }
                sched
            }
            None => Instant::now(),
        };
        stream.write_all(&batch).await.unwrap();

        let mut replies = 0;
        let mut pos = 0;
        while replies < cfg.batch {
            while replies < cfg.batch {
                match parse_one(&acc[pos..]) {
                    Some((is_err, used)) => {
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
            if replies == cfg.batch { break; }

            match timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
                Err(_) => {
                    eprintln!("ERROR: read timeout (round {round}, {replies}/{} replies)", cfg.batch);
                    std::process::exit(1);
                }
                Ok(Ok(0)) => {
                    eprintln!("ERROR: connection closed by server (round {round}, {replies}/{} replies)", cfg.batch);
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

    let total = rounds * cfg.batch;
    print_summary(&cfg, wall, total, oks, errors);
    if let Some(e) = &first_error {
        println!("first error: {e:?}");
    }

    lats.sort_unstable();
    let avg = lats.iter().map(|&l| l as u64).sum::<u64>() as f64 / lats.len().max(1) as f64;
    print_latency_table(&lats, avg);
    print_histogram(&lats);
}
