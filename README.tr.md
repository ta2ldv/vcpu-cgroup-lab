<div align="right">

[English](README.md) | **Türkçe**

</div>

# vCPU & cgroup Lab

CPU sanallaştırma ve kaynak kontrolü üzerine uygulamalı bir lab: Linux cgroup v2 deneyleri, Rust yük üreteçleri ve Kubernetes CPU requests/limits — CPU zamanının donanım thread'lerinden container quota'larına, her katmanda nasıl paylaştırıldığının izi.

Buradaki her deney gerçek bir makinede koşuldu (AWS EC2 `t3.large`, Ubuntu, cgroup v2); gösterilen çıktılar idealize sayılar değil, gerçek ölçümlerdir.

## Müfredat

| # | Bölüm | Cevapladığı soru | Durum |
|---|------|------------------|-------|
| 1 | [Temel kavramlar](#bölüm-1--temel-kavramlar) | Core, hyperthread, vCPU nedir — kim kimi zamanlar? | ✅ |
| 2 | [cgroup v2 elle](#bölüm-2--cgroup-v2-elle) | Kernel CPU zamanını nasıl dilimler, bunu nasıl canlı izlerim? | ✅ |
| 3 | Rust ile yük üreteci | Thread sayısı, concurrency ve parallelism vCPU'larla nasıl etkileşir — tahminle değil ölçümle? | 🔜 |
| 4 | Kubernetes requests/limits | `requests`/`limits` cgroup dosyalarına nasıl çevrilir, `available_parallelism()` neden yalan söyler? | 🔜 |

---

# Bölüm 1 — Temel kavramlar

## 1.1 "CPU" denen dört katman

Herkes "CPU" derken dört farklı şeyden bahseder:

| Katman | Birim | Yöneten |
|---|---|---|
| Donanım | physical core | — |
| Donanım | logical CPU (hyperthread) | CPU'nun kendisi |
| Sanallaştırma | **vCPU** | hypervisor (KVM / Xen / Nitro) |
| Container | CPU quota | Linux cgroup (CFS scheduler) |

Akılda tutmalık model:

> **core** = kas · **vCPU** = o kası kullanma sıran · **cgroup quota** = sıra sendeyken kası ne kadar tutabileceğin.

## 1.2 Core ve SMT (Hyper-Threading)

- **Core** gerçek bir yürütme motorudur: aynı anda tek instruction stream çalıştırır.
- **SMT** (*Simultaneous Multi-Threading*; Intel'in ticari adı: Hyper-Threading) bir core'a **iki architectural register set** verir, execution unit'ler ortaktır. Bir thread takılınca (örn. cache miss) core diğerini çalıştırır.
- **Register set**: CPU içindeki bir avuç ultra hızlı hücre — thread'in anlık durumu: hangi instruction'da, elindeki değerler ne. **Execution unit**: işi fiilen yapan devreler (aritmetik için ALU, memory için load/store unit).
- Benzetme: **bir mutfak (execution units), iki sipariş panosu (register sets)**. Aşçı bir siparişin malzemesini beklerken diğer panodaki işi yapar. İki pano ≠ iki mutfak: SMT 2× değil, kabaca **+%20–30 throughput** kazandırır.
- Linux her hyperthread'i ayrı bir logical CPU olarak gösterir.

## 1.3 vCPU gerçekte nedir

**vCPU bir donanım parçası değildir. Hypervisor'ın host CPU'larına zamanladığı bir thread'dir.**

- Hypervisor için VM'inin her vCPU'su kendi scheduler'ında sıradan bir task'tır — AWS'de (KVM tabanlı Nitro) senin "CPU'n", kelimenin tam anlamıyla başka bir Linux kernel'inin run queue'sundaki bir thread'dir.
- Çoğu AWS instance tipinde **1 vCPU = 1 hyperthread**. Yani `t3.large` = 2 vCPU = **1 physical core**.
- On-prem sanallaştırmada **overcommit** yaygındır: 8 core'lu bir host, VM'lere toplam 40 vCPU dağıtabilir. Çalışır, çünkü VM'ler nadiren aynı anda yüklüdür. vCPU bir *çalışma hakkıdır*, *garanti değildir*.

## 1.4 Guest OS ne görür

- Guest kernel vCPU'ları sıradan CPU sanır: `nproc` vCPU sayısını basar.
- Sanallaştırma tek dürüst metrikten sızar: **steal time** (`top`'ta `%st`) — "vCPU'm çalışmaya hazırdı ama hypervisor fiziksel CPU vermedi." Hypervisor seviyesinde bir kavramdır, cgroup ile ilgisi yoktur.
- Steal'i gözlemenin en kolay yeri burstable instance'lardır (t3 vb.): CPU credit'leri bitir (Standard modda), `%st` yükselsin.

## 1.5 Kubernetes'in "cpu" birimi (ön izleme)

- `cpu: 1` = **period başına bir vCPU'luk zaman**, tahsis edilmiş bir core değil.
- `requests` → *ağırlık* (kavga çıktığında payın); `limits` → *tavan* (quota → throttling).
- Tuzak: pod içinde `nproc` hâlâ **node'un** vCPU sayısını basar — limit ona görünmez. Rust'ın `available_parallelism()`'inin yanıltmasının sebebi budur (Bölüm 4).

## 1.6 Komutlar

| Komut | Ne söyler |
|---|---|
| `lscpu` | CPU **topolojisi**: socket, socket başına core, core başına thread, model, cache'ler |
| `lscpu -e` | Her logical CPU'ya bir satır, `CORE` sütunuyla — hangi ikilinin aynı core'u paylaştığını gösterir |
| `nproc` | **Logical** CPU sayısı, başka hiçbir şey |
| `top` | Process başına canlı `%CPU`; üst satırda `%st` (steal) ve `id` (idle) |
| `cat /sys/fs/cgroup/cgroup.controllers` | Varsa ⇒ cgroup **v2**; içeriği = kernel'in bölebildiği kaynaklar |
| `mount \| grep cgroup` | Hangi filesystem tipi nereye mount edilmiş (`cgroup2` ⇒ v2) |

## 1.7 Gerçek bir makineyi okumak (t3.large)

```
$ lscpu | head -20
CPU(s):                2
Thread(s) per core:    2        ← SMT açık
Core(s) per socket:    1        ← 1 socket × 1 core = 1 physical core
Socket(s):             1
Model name:            Intel(R) Xeon(R) Platinum 8259CL CPU @ 2.50GHz
Hypervisor vendor:     KVM      ← bir VM'iz, host KVM koşuyor (Nitro)
Flags:                 ... ht ... hypervisor ...   ← 'hypervisor' biti: kernel sanallaştırıldığını biliyor
```

Çıkarımlar:

| Soru | Cevap | Kanıt |
|---|---|---|
| Kaç physical core? | **1** | `Socket(s) × Core(s) per socket = 1 × 1` |
| SMT açık mı? | **evet** | `Thread(s) per core: 2` |
| Kaç logical CPU (vCPU)? | **2** | `CPU(s): 2`, `nproc → 2` ile teyitli |
| Sanallaştırılmış mıyız? | **evet, KVM** | `Hypervisor vendor` + `hypervisor` flag'i |

```
$ cat /sys/fs/cgroup/cgroup.controllers
cpuset cpu io memory hugetlb pids rdma misc
```

Dosya var → makine **cgroup v2** kullanıyor; Bölüm 2 için gereken `cpu` + `cpuset` controller'ları mevcut.

---

# Bölüm 2 — cgroup v2 elle

Kubernetes'teki `limits: cpu: 500m`, perde arkasında bir cgroup dizinine yazılan küçük bir dosyadır. Bu bölümde perdeyi kaldırıp o yazma işlemini elle yapıyoruz — Kubernetes'e vardığımızda ortada sihir kalmasın diye.

## 2.1 Kernel ile dosyalar üzerinden konuşmak

`/sys/fs/cgroup`, **diskte duran dosyalar değildir**. `/proc` gibi bir *pseudo-filesystem*'dir: kernel'in dosya biçiminde dışa açtığı kancalar.

- Dosya **okumak** = kernel'i o an sorgulamak (`cat cpu.stat` sayaçları o anda hesaplar).
- Dosyaya **yazmak** = komut vermek (`echo "50000 100000" > cpu.max` limiti anında uygular).

Bu, Unix'in *everything is a file* felsefesidir — özel API ya da syscall gerekmez; `cat` ve `echo` tüm alet çantasıdır.

```
$ mount | grep cgroup
cgroup2 on /sys/fs/cgroup type cgroup2 (rw,nosuid,nodev,noexec,relatime,...)
```

`cgroup2` filesystem'i `/sys/fs/cgroup`'a mount edilmiş. cgroup yalnızca *bu filesystem'in içinde* yaratılabilir — başka bir yerde `mkdir`, diskte sıradan boş bir dizindir.

## 2.2 Yerleşim: dizinler cgroup'tur

```
$ ls /sys/fs/cgroup/
cgroup.controllers  cgroup.procs  cgroup.subtree_control  cpu.stat ...   ← kontrol dosyaları (bu cgroup = root)
init.scope/  system.slice/  user.slice/                                  ← çocuk cgroup'lar (systemd yarattı)
```

- Her **dizin** bir cgroup'tur; dizinleri iç içe koymak cgroup'ları bir **ağaca** dizer.
- Makinen zaten bu ağacın içinde koşuyor: systemd servisleri `system.slice/`'a, SSH oturumunu `user.slice/`'a koyar.
- Ağacın içinde `mkdir` = *cgroup yarat*. Kernel yeni dizini kontrol dosyalarıyla anında döşer. `rmdir` yok eder.

```
$ sudo mkdir /sys/fs/cgroup/lab
$ ls /sys/fs/cgroup/lab/ | head -5
cgroup.controllers
cgroup.events
cgroup.freeze
cgroup.kill
cgroup.max.depth        ← bunları kimse yaratmadı; kernel, mkdir anında yarattı
```

## 2.3 `subtree_control` kapısı

```
$ cat /sys/fs/cgroup/cgroup.controllers        # bu kernel'de ne var
cpuset cpu io memory hugetlb pids rdma misc
$ cat /sys/fs/cgroup/cgroup.subtree_control    # root çocuklarına ne açmış
cpuset cpu io memory pids
```

Bir cgroup'ta `cpu.*` dosyalarının var olması için **parent'ının `subtree_control`'ünde `cpu` yazmalıdır**. Root'ta açık gelir (systemd sağ olsun); kendi parent cgroup'larında kendin açmalısın:

```bash
echo "+cpu +cpuset" | sudo tee /sys/fs/cgroup/<parent>/cgroup.subtree_control
```

(`+` açar, `-` kapatır.) Bunu unutmak, klasik "file not found: cpu.max" hatasının sebebidir.

İlgili v2 kuralı (**no internal processes**): bir cgroup çocuklarına controller açtıysa, process'ler yalnızca **yapraklarında** yaşayabilir, parent'ın kendisinde asla. Kubernetes'in pod'ları ağacın en dibinde tutmasının sebebi budur.

## 2.4 cgroup dosyalarını okuma rehberi

Sürekli okuyacağın dört dosya. Tek cümlelik özet:

> **`cpu.max` = kural · `cgroup.procs` = mahkûmlar · `/proc/<pid>/cgroup` = mahkûmun künyesi · `cpu.stat` = sicil defteri.**

### `cpu.max` — kural

```
$ cat /sys/fs/cgroup/lab/cpu.max
max 100000
```

Format: `<quota> <period>`, ikisi de mikrosaniye. Zaman `period` µs'lik tekrar eden pencerelere bölünür; her pencerede cgroup'un process'leri **tüm CPU'lar üzerindeki toplamda** en fazla `quota` µs CPU zamanı harcayabilir. Bütçe bitince kernel onları sonraki pencereye kadar dondurur — *throttling* budur.

| Değer | Anlamı |
|---|---|
| `max 100000` | sınırsız (default; `max` bir anahtar kelime) — CPU limiti olmayan pod |
| `50000 100000` | her 100 ms'de 50 ms = **0.5 vCPU** — Kubernetes'in `500m` için yazdığının ta kendisi |
| `200000 100000` | **2 vCPU** |
| `5000 10000` | yine 0.5 vCPU ama 10 ms'lik pencerelerle → daha kısa donmalar, daha az latency etkisi |

Period'u sen seçersin (kernel 1 ms – 1 s kabul eder; 100 ms hem kernel'in hem Kubernetes'in default'u). **Oran ortalama hızı, period donma granülaritesini belirler.**

```
|—— pencere 1 (100 ms) ——|—— pencere 2 (100 ms) ——|
[■■■■ 50ms koş ][ donuk ][■■■■ 50ms koş ][ donuk ]     ← cpu.max = 50000 100000
```

`top` böyle bir process'i %50 gösterir — ama gerçek şudur: *zamanın yarısında tam hızda* koşuyor.

### `cgroup.procs` — mahkûmlar

```
$ cat /sys/fs/cgroup/lab/cgroup.procs
11254
```

Satır başına bir PID: şu an bu cgroup'ta kim var. İçine PID yazmak process'i **taşır**; limitler o andan itibaren uygulanır. Restart yok, sinyal yok — process olan biteni fark etmez.

### `/proc/<pid>/cgroup` — mahkûmun künyesi

```
$ cat /proc/11254/cgroup
0::/lab
```

Aynı gerçeğin process tarafından görünüşü: cgroup root'una göre yolu. `0::/` = root'ta; `0::/user.slice/...` = sıradan bir oturum process'i.

### `cpu.stat` — sicil defteri

```
$ cat /sys/fs/cgroup/lab/cpu.stat
usage_usec 25900043      ← tüketilen toplam CPU zamanı (µs) ≈ 25.9 s
user_usec 25877331       ←   ... user code'da geçen
system_usec 22712        ←   ... kernel içinde geçen (syscall'lar)
nr_periods 665           ← kaç quota penceresi geçti
nr_throttled 663         ← kaçında grup donduruldu
throttled_usec 40255281  ← toplam donuk süre ≈ 40 s
```

- **Throttle oranı** = `nr_throttled / nr_periods` → burada 663/665 ≈ **%99.7**: bu grup neredeyse her pencerede duvara çarpıyor.
- Sayaçlar cgroup yaratıldığından beri **kümülatiftir** — hız için iki kez okuyup fark al.
- Prod sağlık kuralı: sürekli tırmanan `nr_throttled`, limitin dar olduğu anlamına gelir.

## 2.5 Deney 1 — `cpu.max` ile throttling

**Hedef:** CPU'ya aç bir process'i cgroup'a koyup yarım vCPU ile sınırlamak ve kernel'in bu tavanı uygulayışını izlemek — hem canlı, hem sicil defterinde.

```bash
# 1. Hücreyi yarat
sudo mkdir /sys/fs/cgroup/lab

# 2. Yükü başlat, PID'i not et
bash -c 'while :; do :; done' & echo "PID: $!"

# 3. Kuralı koy: her 100 ms'de 50 ms = yarım vCPU
echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max

# 4. Process'i hücreye taşı (limit o an biner)
echo <PID> | sudo tee /sys/fs/cgroup/lab/cgroup.procs

# 5. İzle: top'ta %CPU ≈ 50; itiraf cpu.stat'ta
top -p <PID>
cat /sys/fs/cgroup/lab/cpu.stat        # nr_throttled artıyor mu?

# 6. Canlı oyna — restart gerekmez:
echo "20000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max   # → ~%20
echo "max 100000"   | sudo tee /sys/fs/cgroup/lab/cpu.max   # → %100'e geri

# 7. Temizlik
kill <PID>
sudo rmdir /sys/fs/cgroup/lab
```

Ölçülen sonuç (adım 5–6):

```
  PID USER  ...  %CPU  COMMAND
11254 ubuntu ... 100.0  bash        ← cgroup'a girmeden önce
11254 ubuntu ...  50.0  bash        ← girdikten sonra; adım 6'dan sonra 20.0

$ cat /sys/fs/cgroup/lab/cpu.stat
nr_periods 665
nr_throttled 663          ← 665 pencerenin 663'ünde donduruldu
throttled_usec 40255281   ← ~40 s donuk geçmiş
```

Adım 3–4'ün sırası önemli değildir: limit *hücreye* aittir; içeri giren, girdiği an kurala tabidir.

## 2.6 Deney 2 — `cpu.weight`, pay

**`cpu.max` tavandır ("asla bundan fazlası yok"). `cpu.weight` paydır ("*kavga çıkarsa* payın bu; ortalık boşsa hepsini al").**

- Aralık 1–10000, default **100**. Mutlak değil görelidir: weight 300, weight 100'e karşı 3'e 1 alır.
- Boş CPU'da weight hükümsüzdür — weight 1 bile %100 alır. Weight asla throttle etmez, `nr_throttled` artırmaz.
- Kubernetes: `requests: cpu` → `cpu.weight`'e çevrilir. Bu deney, "requests vs limits"in kernel seviyesindeki halidir.

**Tasarım problemi:** 2 vCPU'muz var. İki busy loop ayrı vCPU'lara oturur ve hiç kavga etmez — weight ise yalnızca kavgada hakemlik yapar. Kavgayı, iki cgroup'u **`cpuset.cpus`** ile CPU 0'a kilitleyerek zorluyoruz (üçüncü controller: `cpuset` = *nerede* koşabilirsin, `cpu.max` = *ne kadar*, `cpu.weight` = *kuyrukta kim kazanır*).

```bash
# 1. İki hücre
sudo mkdir /sys/fs/cgroup/w100-lab /sys/fs/cgroup/w300-lab

# 2. İkisini de CPU 0'a kilitle → kavga garanti
echo 0 | sudo tee /sys/fs/cgroup/w100-lab/cpuset.cpus
echo 0 | sudo tee /sys/fs/cgroup/w300-lab/cpuset.cpus

# 3. Paylar: w100-lab default'ta kalır (100), w300-lab'e 300
echo 300 | sudo tee /sys/fs/cgroup/w300-lab/cpu.weight

# 4. İki özdeş yük
bash -c 'while :; do :; done' & echo "PID_100: $!"
bash -c 'while :; do :; done' & echo "PID_300: $!"

# 5. Her biri kendi hücresine
echo <PID_100> | sudo tee /sys/fs/cgroup/w100-lab/cgroup.procs
echo <PID_300> | sudo tee /sys/fs/cgroup/w300-lab/cgroup.procs

# 6. İkisini izle
top -p <PID_100> -p <PID_300>

# 7. Weight'in limit olmadığının kanıtı: 300'ü öldür, 100'ü izle
kill <PID_300>

# 8. Temizlik
kill <PID_100>
sudo rmdir /sys/fs/cgroup/w100-lab /sys/fs/cgroup/w300-lab
```

Ölçülen sonuç (adım 6) — 100:300 oranı, canlı:

```
%Cpu(s): 50.6 us, ... 48.6 id     ← makine toplamı: yarısı dolu — CPU 1 boş duruyor (cpuset!)
  PID USER  ...  %CPU  COMMAND
11664 ubuntu ...  75.1  bash      ← weight 300
11663 ubuntu ...  24.9  bash      ← weight 100
```

Adım 7'den sonra hayatta kalan **%100'e** fırladı: kavga bitti, payın bölecek bir şeyi kalmadı. Bir `cpu.max` tavanı olsaydı makine bomboşken bile tavanında kalırdı — requests ile limits arasındaki farkın tamamı budur.

## 2.7 Deney 3 — hiyerarşi: üç perdede `tree-lab`

cgroup'lar bir **ağaç** oluşturur ve parent'ın limiti çocuklarının *toplamına* uygulanır. Kubernetes'in düzeni tam olarak budur: `kubepods.slice/` → pod → container. Bu deney iki çocuklu bir ağaç kurar ve üç perde oynar — biri, kendi tahminimizin yanlış çıktığı ve lab'ın en keskin dersini öğreten perde.

```
/sys/fs/cgroup/tree-lab/        ← parent: cpu.max TOPLAMI sınırlar
├── w100/                       ← çocuk, weight 100
└── w300/                       ← çocuk, weight 300
```

**Soru:** 2 vCPU boştayken, iki çocuk ayrı CPU'lara kaçarak parent'ın 0.5 vCPU'luk toplamından kurtulabilir mi? Ve parent'ın pastasını weight mi böler?

```bash
# ── KURULUM ────────────────────────────────────────────
sudo mkdir /sys/fs/cgroup/tree-lab

# Çocuklara cpu VE cpuset'i aç (atlarsan o dosyalar çocuklarda var olmaz)
echo "+cpu +cpuset" | sudo tee /sys/fs/cgroup/tree-lab/cgroup.subtree_control

sudo mkdir /sys/fs/cgroup/tree-lab/w100 /sys/fs/cgroup/tree-lab/w300
ls /sys/fs/cgroup/tree-lab/w100/ | grep cpu      # dosyalar geldi mi, doğrula

# Pasta: tüm alt ağaca yarım vCPU
echo "50000 100000" | sudo tee /sys/fs/cgroup/tree-lab/cpu.max

# Paylar: w100 default (100) kalır, w300'e 300
echo 300 | sudo tee /sys/fs/cgroup/tree-lab/w300/cpu.weight

# İki özdeş yük, her biri bir YAPRAĞA (parent'a yazmak reddedilir)
bash -c 'while :; do :; done' & echo "PID_100: $!"
bash -c 'while :; do :; done' & echo "PID_300: $!"
echo <PID_100> | sudo tee /sys/fs/cgroup/tree-lab/w100/cgroup.procs
echo <PID_300> | sudo tee /sys/fs/cgroup/tree-lab/w300/cgroup.procs

# ── PERDE A: kavgasız quota ────────────────────────────
top -p <PID_100> -p <PID_300>
cat /sys/fs/cgroup/tree-lab/cpu.stat     # throttling PARENT'ta sayılır

# ── PERDE B: ikisini CPU 0'a kilitle — weight uyanır ───
echo 0 | sudo tee /sys/fs/cgroup/tree-lab/w100/cpuset.cpus
echo 0 | sudo tee /sys/fs/cgroup/tree-lab/w300/cpuset.cpus
top -p <PID_100> -p <PID_300>

# ── PERDE C: pastayı büyüt — sahnede yalnız weight ─────
echo "100000 100000" | sudo tee /sys/fs/cgroup/tree-lab/cpu.max
top -p <PID_100> -p <PID_300>

# ── TEMİZLİK (önce çocuklar — dolu dizin rmdir edilmez) ──
kill <PID_100> <PID_300>
sudo rmdir /sys/fs/cgroup/tree-lab/w100 /sys/fs/cgroup/tree-lab/w300
sudo rmdir /sys/fs/cgroup/tree-lab
```

Ölçülen sonuçlar:

| Perde | Kurulum | w100 | w300 | Toplam |
|---|---|---|---|---|
| A | quota ½ vCPU, cpuset yok | **%25** | **%25** | ~%50 |
| B | + ikisi CPU 0'a kilitli | **%12.7** | **%37.3** | ~%50 |
| C | + quota 1 vCPU'ya çıktı | **%25.0** | **%74.7** | ~%100 |

**Perde A — neden 25/25, neden 25/75 değil?** 25/75 tahmin etmiştik ve yanıldık. Process'ler ayrı vCPU'lara kaçtı — ama quota'dan kaçamadılar: **quota CPU başına değil, cgroup'un toplamına yazılır**. İki CPU'da koşmak 50 ms'lik bütçeyi sadece 2× hızla yakar (25 ms'de biter), sonra *ikisi birden* donar. Ama hiç *aynı* CPU'nun kuyruğuna girmedikleri için kavga da çıkmadı — ve **weight yalnızca bir CPU'nun kuyruğundaki kavgada hakemlik yapar**. Kavga yoksa bütçe ilk-gelen-alır usulü tüketilir: eşit bölüşüm.

**Perde B** — aynı 50 ms'lik pasta, ama artık ikisi CPU 0'un kuyruğunda → weight hakem: 100:300 ⇒ tahmin 12.5/37.5, ölçüm 12.7/37.3.

**Perde C** — pasta (100 ms) artık tek CPU'nun verebileceğini aşıyor (CPU 0 100 ms'de en fazla 100 ms verebilir), quota görünmez olur ve CPU'yu yalnız weight böler: 25/75.

> **Pasta, bıçak, masa:** `cpu.max` pastanın boyunu belirler, `cpu.weight` onu kesen bıçaktır, `cpuset` masayı seçer — ve bıçak ancak herkes aynı masaya oturursa çalışır.

Perde A'nın Kubernetes çevirisi bir prod klasiğidir: çok thread'li bir uygulamaya çok core'lu bir node'da `limit: 1` ver; thread'leri CPU'lara yayılır, bütçeyi period'un küçük bir diliminde bitirir ve hep birlikte donar — ortalama CPU "sadece" %100 görünürken latency patlar. Bölüm 3'te tam olarak bunu Rust ile ölçeceğiz.

## 2.8 Bölüm 2'nin özeti

| Kavram | Tek cümle |
|---|---|
| cgroup | Kernel'in yönettiği ağaçta isimli bir hücre; dizinler hücre, dosyalar API |
| `cpu.max` | Tavan: µs cinsinden `quota period`; dondurarak uygulanır (throttling); tüm CPU'lar üzerindeki cgroup toplamına sayılır |
| `cpu.weight` | Pay: çekişmeli CPU'yu kardeşler arasında böler; kavga yoksa hükümsüz; asla throttle etmez |
| `cpuset.cpus` | Yerleşim: hücre hangi logical CPU'ları kullanabilir |
| `subtree_control` | Parent'ın çocuklarına controller açması (`+cpu +cpuset`); onsuz çocuk kontrol dosyaları var olmaz |
| No-internal-process kuralı | Parent controller devrettiyse process'ler yalnız yaprak cgroup'larda yaşar |
| `cpu.stat` | Kümülatif sicil; `nr_throttled/nr_periods` throttle oranındır |
| K8s eşlemesi | `requests` → `cpu.weight`, `limits` → `cpu.max`, pod ağacı → cgroup ağacı |

---

# Bölüm 3 — Rust ile yük üreteci *(yakında)*

Kendi ölçümünü yapan bir yük üreteci: N thread, her biri saniyede yaptığı işi sayar. Planlanan deneyler: 2 vCPU'ya karşı thread sayısı taraması (parallelism duvarı), aynı taramanın throttle'lı cgroup içinde tekrarı ve `std::thread::available_parallelism()` ile gerçeğin karşılaştırması.

# Bölüm 4 — Kubernetes requests & limits *(yakında)*

kubelet'in kurduğu cgroup ağacında gezinti (`kubepods.slice/...`), `requests`/`limits`'in Bölüm 2'deki dosyalara birebir eşlenmesi, OpenShift'te gerçek pod'larda CFS throttling gözlemi ve runtime thread-pool boyutlandırmasının neden cgroup-aware olması gerektiği.
