// ═══════════════════════════════════════════════════════════════════════
// RespPress — step01: ilk temas (Single-Thread PoC'nin ilk hali)
// ═══════════════════════════════════════════════════════════════════════
// Ne yapar:
//   1. localhost:6379'daki Redis'e TEK bir async TCP bağlantısı açar
//   2. SET foo bar komutunu §5.1 kurallarıyla elle kodlayıp gönderir
//   3. Cevabı okur ve ham haliyle basar        → "+OK\r\n"
//   4. GET foo gönderir, cevabı basar          → "$3\r\nbar\r\n"
//   5. main biter → bağlantı kapanır (drop)
//
// Ne YAPMAZ (bilerek):
//   - argüman okumaz, ölçüm yapmaz, pipeline kurmaz, tek bağlantıdır.
//   - amaç tek şeyi kanıtlamak: "Rust'tan RESP konuşabiliyoruz."
//
// Koşu:  cargo build --release && ./target/release/step01
// ═══════════════════════════════════════════════════════════════════════

use tokio::io::{AsyncReadExt, AsyncWriteExt}; // write_all/read metodlarını getiren trait'ler —
use tokio::net::TcpStream;                    // bu use satırı olmadan o metodlar GÖRÜNMEZ

// §5.1'in kuralları tek fonksiyonda: ["SET","foo","bar"] → RESP frame byte'ları
// Generic'tir: ["MSET","k1","v1",...] gibi her komutu yutar.
fn encode(cmd: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();                                       // büyüyebilen byte dizisi
    out.extend_from_slice(format!("*{}\r\n", cmd.len()).as_bytes()); // *N   → "N elemanlı array"
    for part in cmd {
        out.extend_from_slice(format!("${}\r\n", part.len()).as_bytes()); // $uzunluk
        out.extend_from_slice(part.as_bytes());                           // içerik
        out.extend_from_slice(b"\r\n");                                   // her satır CRLF ile biter
    }
    out
}

#[tokio::main]
async fn main() {
    // nc localhost 6379'un Rust'ı: bağlanana kadar worker'ı bırakır (gerçek .await)
    let mut stream = TcpStream::connect("127.0.0.1:6379").await.unwrap();
    println!("connected to {}", stream.peer_addr().unwrap());

    let mut buf = [0u8; 4096]; // cevapların okunacağı buffer

    let set = encode(&["SET", "foo", "bar"]);   // frame'i BELLEKTE kur (henüz network yok)
    stream.write_all(&set).await.unwrap();      // hepsini tele it (parça parça gitse de ısrar eder)
    let n = stream.read(&mut buf).await.unwrap();               // cevabı bekle; n = gelen byte sayısı
    println!("SET reply: {:?}", String::from_utf8_lossy(&buf[..n])); // {:?} → \r\n'ler görünür basılır

    let get = encode(&["GET", "foo"]);
    stream.write_all(&get).await.unwrap();
    let n = stream.read(&mut buf).await.unwrap();
    println!("GET reply: {:?}", String::from_utf8_lossy(&buf[..n]));
}
