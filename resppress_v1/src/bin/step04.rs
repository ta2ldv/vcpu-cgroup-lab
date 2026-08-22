// ═══════════════════════════════════════════════════════════════════════
// RespPress — step04: gerçek reply parser + karışık workload + hata sayacı
// ═══════════════════════════════════════════════════════════════════════
// Ne yapar:
//   1. batch'i SET,GET,SET,GET,... dönüşümüyle kurar (50/50) — cevap boyları
//      artık DEĞİŞKEN: +OK\r\n (5B) ile $3\r\nbar\r\n (9B) nöbetleşe gelir.
//      step03'ün "byte say" numarası bu dünyada imkânsız; parser şart.
//   2. Cevapları RESP dilbilgisiyle TEK TEK ayıklar (parse_one) — artık
//      byte değil CEVAP sayıyoruz.
//   3. Hata politikası değişti: fail-fast emekli — "-ERR ..." görülünce
//      ÇÖKMEZ, errors sayacı artar, ilk hatanın örneği saklanır, koşu sürer.
//      (Hatayı saymayan benchmark yalancıdır; hata artık veri.)
//   4. Read timeout (5 sn): suskun server'da sonsuza dek beklemek de bug'dı,
//      kapandı. Kopuş tespiti (read()==0) step03'ten devam.
//   5. Rapor: throughput satırı + "replies: X ok, Y errors (first error: ...)"
//
// Ölçümler (t3.large, localhost):
//   1000/10   → 102-150k req/s  (10 ms'lik koşu; gürültü baskın, normaldir)
//   5M/1000   → 1.48-1.54M req/s, "5000000 ok, 0 errors" (iki koşu)
//   Kıyas: step03 aynı parametrede 1.29-1.35M idi → parser MALİYETSİZ.
//   (Şerh: workload da değişti — SET-only → 50/50; hızlanmanın muhtemel
//    sebebi GET'in Redis'e SET'ten ucuz olması. İki değişken, tek fark —
//    "parser hızlandırdı" diyemeyiz, "yavaşlatmadı" diyebiliriz.)
//
// Bilinen sınırlar (sonraki adımlara):
//   - latency ölçümü yok (step05: p50/p99 + histogram)
//   - sabit rate yok (step06), komut karışımı düğmesi yok (step07)
//   - GET içerik doğrulaması yok (parser geçerliliğe bakar, içeriğe değil)
//
// Koşu:  cargo run --release -- 5000000 1000
// Sabotaj testi: "SET" → "SETX" yap → çökmez; sonunda
//   "replies: N/2 ok, N/2 errors (first error: \"-ERR unknown command...\")"
// ═══════════════════════════════════════════════════════════════════════

use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

// ── Frame kodlayıcı (step01'den beri değişmedi) ─────────────────────────
// ["SET","foo","bar"] → *3\r\n$3\r\nSET\r\n$3\r\nfoo\r\n$3\r\nbar\r\n
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

// ── Yardımcı: buf[from..] içindeki ilk \r\n'in konumu ───────────────────
// Bulamazsa None = "satır henüz tamamlanmadı, daha byte lazım".
fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    buf[from..].windows(2).position(|w| w == b"\r\n").map(|i| i + from)
}

// ── GERÇEK REPLY PARSER ─────────────────────────────────────────────────
// Akıştan RESP dilbilgisiyle TAM BİR cevap ayıklar.
//   Some((hata_mı, tüketilen_byte)) → tam cevap bulundu
//   None                            → cevap eksik (kısmi frame): OKUMAYA DEVAM
// Kısmi frame TCP'nin doğasıdır: read() cevabı ortadan bölebilir
// ("$3\r\nba" bu read'de, "r\r\n" sonrakinde). Parser'ın "durumu",
// buffer'da bekleyen byte'ların kendisidir — ekstra state tutulmaz.
fn parse_one(buf: &[u8]) -> Option<(bool, usize)> {
    match *buf.first()? {                          // ilk byte = tür (5.1 tablosu)
        b'+' | b':' => {                           // simple string / integer
            let end = find_crlf(buf, 1)?;
            Some((false, end + 2))
        }
        b'-' => {                                  // error — aynı biçim, hata bayrağıyla
            let end = find_crlf(buf, 1)?;
            Some((true, end + 2))
        }
        b'$' => {                                  // bulk string: $uzunluk\r\n içerik\r\n
            let hdr_end = find_crlf(buf, 1)?;
            let len: i64 = std::str::from_utf8(&buf[1..hdr_end]).ok()?.parse().ok()?;
            if len < 0 {
                return Some((false, hdr_end + 2)); // $-1\r\n = null (key yok) — hata DEĞİL
            }
            let total = hdr_end + 2 + len as usize + 2;
            if buf.len() >= total { Some((false, total)) } else { None }
        }
        b'*' => {                                  // array: her eleman özyinelemeyle
            let hdr_end = find_crlf(buf, 1)?;
            let count: i64 = std::str::from_utf8(&buf[1..hdr_end]).ok()?.parse().ok()?;
            let mut pos = hdr_end + 2;
            let mut any_err = false;
            for _ in 0..count.max(0) {
                let (e, used) = parse_one(&buf[pos..])?;  // eleman eksikse None yukarı taşar
                any_err |= e;
                pos += used;
            }
            Some((any_err, pos))
        }
        _ => Some((true, 1)),                      // protokol çöpü: hata say, 1 byte ilerle
    }
}

#[tokio::main]
async fn main() {
    // ── Argümanlar + koşu kimliği ───────────────────────────────────────
    let n: usize          = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let batch_size: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(1);
    println!("PID: {}", std::process::id());
    println!("Sending {n} messages (SET/GET mix 50/50, batch size {batch_size}) to 127.0.0.1:6379 ...");

    let mut stream = TcpStream::connect("127.0.0.1:6379").await.unwrap();
    let mut buf = [0u8; 65536];

    // ── Karışık batch kurulumu ──────────────────────────────────────────
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

    // acc = akış buffer'ı: read'ler ekler, parser tüketir.
    let mut acc: Vec<u8> = Vec::new();

    let start = Instant::now();
    for round in 0..rounds {
        stream.write_all(&batch).await.unwrap();   // batch tek write'ta tele

        let mut replies = 0;
        let mut pos = 0;
        while replies < batch_size {
            // eldeki byte'lardan parse edilebilen her cevabı ayıkla
            while replies < batch_size {
                match parse_one(&acc[pos..]) {
                    Some((is_err, used)) => {
                        if is_err {
                            errors += 1;           // SAY VE DEVAM ET
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
                    None => break,                 // eksik cevap → network'ten devam
                }
            }
            if replies == batch_size { break; }

            // ── Okuma: 5 sn timeout + kopuş + IO hatası tespiti ─────────
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
        acc.drain(..pos);                          // tüketileni at, buffer büyümesin
    }
    let wall = start.elapsed().as_secs_f64();

    // ── Rapor ───────────────────────────────────────────────────────────
    let total = rounds * batch_size;
    println!("{total} requests in {wall:.2} s  ->  {:.0} req/s  (avg {:.2} µs/req)",
             total as f64 / wall,
             wall / total as f64 * 1_000_000.0);
    println!("replies: {oks} ok, {errors} errors{}",
             match &first_error {
                 Some(e) => format!("  (first error: {e:?})"),
                 None => String::new(),
             });
}
