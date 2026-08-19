<div align="right">

[English](README.md) | **Türkçe**

</div>

# vCPU & cgroup Lab

CPU sanallaştırma ve kaynak kontrolü üzerine uygulamalı bir lab: Linux cgroup v2 deneyleri, Rust yük üreteçleri ve Kubernetes CPU requests/limits — CPU zamanının donanım thread'lerinden container quota'larına, her katmanda nasıl paylaştırıldığının izi.

Buradaki her deney gerçek bir makinede koşuldu (AWS EC2 `t3.large`, Ubuntu, cgroup v2); gösterilen çıktılar idealize sayılar değil, gerçek ölçümlerdir.

## İçindekiler

- [Bölüm 1 — Temel kavramlar](#bölüm-1--temel-kavramlar)
  - [1.1 "CPU" denen dört katman](#11-cpu-denen-dört-katman)
  - [1.2 Core ve SMT (Hyper-Threading)](#12-core-ve-smt-hyper-threading)
  - [1.3 vCPU gerçekte nedir](#13-vcpu-gerçekte-nedir)
  - [1.4 Guest OS ne görür](#14-guest-os-ne-görür)
  - [1.5 Kubernetes'in "cpu" birimi (ön izleme)](#15-kubernetesin-cpu-birimi-ön-izleme)
  - [1.6 Komutlar](#16-komutlar)
  - [1.7 Gerçek bir makineyi okumak (t3.large)](#17-gerçek-bir-makineyi-okumak-t3large)
- [Bölüm 2 — cgroup v2 elle](#bölüm-2--cgroup-v2-elle)
  - [2.1 Kernel ile dosyalar üzerinden konuşmak](#21-kernel-ile-dosyalar-üzerinden-konuşmak)
  - [2.2 Yerleşim: dizinler cgroup'tur](#22-yerleşim-dizinler-cgrouptur)
  - [2.3 `subtree_control` kapısı](#23-subtree_control-kapısı)
  - [2.4 cgroup dosyalarını okuma rehberi](#24-cgroup-dosyalarını-okuma-rehberi)
  - [2.5 Deney 1 — `cpu.max` ile throttling](#25-deney-1--cpumax-ile-throttling)
  - [2.6 Deney 2 — `cpu.weight`, pay](#26-deney-2--cpuweight-pay)
  - [2.7 Deney 3 — hiyerarşi: üç perdede `tree-lab`](#27-deney-3--hiyerarşi-üç-perdede-tree-lab)
  - [2.8 Bölüm 2'nin özeti](#28-bölüm-2nin-özeti)
- [Bölüm 3 — Rust ile yük üreteci](#bölüm-3--rust-ile-yük-üreteci)
  - [3.1 VM'de Rust toolchain kurulumu](#31-vmde-rust-toolchain-kurulumu)
  - [Deney 3.2 — ilk ölçüm: saat sorunu](#deney-32--ilk-ölçüm-saat-sorunu)
  - [Deney 3.3 — temiz ölçüm, N thread](#deney-33--temiz-ölçüm-n-thread)
  - [Deney 3.4 — thread taraması: parallelism duvarı](#deney-34--thread-taraması-parallelism-duvarı)
  - [Deney 3.5 — thread × quota matrisi](#deney-35--thread--quota-matrisi)
  - [Deney 3.6 — stall'lar: matrisin göremediği acı](#deney-36--stalllar-matrisin-göremediği-acı)
  - [Deney 3.7 — "kaç CPU var?" sorusuna kim dürüst cevap verir](#deney-37--kaç-cpu-var-sorusuna-kim-dürüst-cevap-verir)
- [Bölüm 4 — Async Rust: tokio ve vCPU](#bölüm-4--async-rust-tokio-ve-vcpu-devam-ediyor)
  - [4.1 Cargo'nun dönüşü — proje kurulumu](#41-cargonun-dönüşü--proje-kurulumu)
  - [4.2 tokio nasıl zamanlar — cooperative, `.await`, task queue'lar](#42-tokio-nasıl-zamanlar--cooperative-await-task-queuelar)
  - [Deney 4.3 — tokio kaç worker açar?](#deney-43--tokio-kaç-worker-açar)
- [Bölüm 5 — Kubernetes requests & limits](#bölüm-5--kubernetes-requests--limits-yakında)
- [Bölüm 6 — Performans lab'ı: Redis & Dragonfly boyutlandırma](#bölüm-6--performans-labı-redis--dragonfly-boyutlandırma-yakında)

## Müfredat

| # | Bölüm | Cevapladığı soru | Durum |
|---|------|------------------|-------|
| 1 | [Temel kavramlar](#bölüm-1--temel-kavramlar) | Core, hyperthread, vCPU nedir — kim kimi zamanlar? | ✅ |
| 2 | [cgroup v2 elle](#bölüm-2--cgroup-v2-elle) | Kernel CPU zamanını nasıl dilimler, bunu nasıl canlı izlerim? | ✅ |
| 3 | [Rust ile yük üreteci](#bölüm-3--rust-ile-yük-üreteci) | Thread sayısı, concurrency ve parallelism vCPU'larla nasıl etkileşir — tahminle değil ölçümle? | ✅ |
| 4 | [Async Rust (tokio)](#bölüm-4--async-rust-tokio-ve-vcpu-devam-ediyor) | Async task'lar thread'lerin üstüne ne katar — worker pool, vCPU ve cgroup limitleriyle nasıl etkileşir? | ⏳ |
| 5 | [Kubernetes requests/limits](#bölüm-5--kubernetes-requests--limits-yakında) | `requests`/`limits` cgroup dosyalarına nasıl çevrilir, tüm sync/async iş yükleri bunların altında nasıl davranır? | 🔜 |
| 6 | [Performans lab'ı (Redis & Dragonfly)](#bölüm-6--performans-labı-redis--dragonfly-boyutlandırma-yakında) | Zıt mimarili iki engine için *doğru* CPU kısıtları ne — VM'de ve OpenShift'te, ölçümle kanıtlı? | 🔜 |

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
- Tuzak: pod içinde `nproc` hâlâ **node'un** vCPU sayısını basar — limit ona görünmez. Dilin runtime'ı bu hatayı tekrarlıyor mu, düzeltiyor mu — Deney 3.7'de ölçülüyor.

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

[↑ Go back to TOC](#i̇çindekiler)

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

Period'dan *büyük* bir quota nasıl sığar — mesela `150000 100000`, 100 ms'lik pencerede 150 ms? Çünkü **quota CPU başına değil, cgroup'un tüm CPU'lardaki toplamına sayılır**. 2 vCPU'lu makinede bir pencere en fazla 2 × 100 = 200 ms CPU zamanı sunar; aynı anda koşan iki thread 150 ms'lik bütçeyi 75 ms duvar saatinde yakar:

```
pencere: |———————— 100 ms duvar saati ————————|
CPU 0:   [■■■■■■■ 75 ms koştu ■■■■■■■][ donuk ]
CPU 1:   [■■■■■■■ 75 ms koştu ■■■■■■■][ donuk ]
                                       toplam: 75+75 = 150 ms — bütçe bitti
```

Yani `150000 100000` = 150/100 = **1.5 vCPU'luk zaman hakkı**. Ezber kalıbı: **soldakini sağdakine böl, vCPU sayısını bul.**

#### Tavan: sol değerin anlamlı maksimumu nedir?

İsimlendirme, bir kez ve net — `a b` girdisinde:

```
a = quota   → BÜTÇE: cgroup'un pencere başına harcayabileceği CPU zamanı (µs)
b = period  → PENCERE: bir muhasebe turunun duvar saati uzunluğu (µs)
```

Bütçe **logical CPU'larda** harcanır — kernel'in üstüne iş dizdiği birimler. Sayıları donanımdan gelir:

```
logical CPU = physical core × SMT katsayısı
```

(§1.2'den SMT özeti: tek fiziksel core'un 2+ komple register seti taşıması; OS onu 2+ CPU olarak görür, execution devreleri ortaktır. x86'da katsayı 2; IBM POWER'da 4–8; Apple M / Graviton'da SMT yok, katsayı 1.)

Her logical CPU bir pencerede en fazla `b` µs katkı verebilir — duvar saatinden fazla koşamaz. Tavan buradan:

```
pencere başına harcanabilir maks quota:   a_max = b × logical CPU sayısı
```

Bu lab'ın t3.large'ında, çizimle:

```
                    ┌─ physical core 0 ─┐
                    │  HT-A       HT-B  │        1 core × SMT 2 = 2 logical CPU
                    └───┬───────────┬───┘
                        │           │
pencere (b = 100 ms):   │           │
CPU 0 = HT-A:  [■■■ en çok 100 ms ■■■]  ┐
CPU 1 = HT-B:  [■■■ en çok 100 ms ■■■]  ┴→  a_max = 2 × 100 ms = 200 ms
```

Sayısal örnekler, hepsinde `b = 100000` (100 ms):

| Makine | core | SMT | logical CPU | tavan `a_max` | yani bunun ötesi anlamsız: |
|---|---|---|---|---|---|
| t3.large (bu lab) | 1 | 2 | 2 | 200 ms | `200000 100000` |
| 4 core SMT'li Xeon | 4 | 2 | 8 | 800 ms | `800000 100000` |
| Graviton, 8 core | 8 | 1 | 8 | 800 ms | `800000 100000` |
| POWER9, 4 core SMT8 | 4 | 8 | 32 | 3200 ms | `3200000 100000` |

Formüle üç dipnot:

1. Tavanın **üstünde** quota yazmak geçerlidir — kernel t3'te `800000 100000`'i kabul eder — ama grup fiziksel olarak 200 ms'den fazlasını harcayamaz; tavanın ötesi `max` gibi davranır.
2. Çarpan, **bu cgroup'un erişebildiği** logical CPU sayısıdır: `cpuset.cpus 0` pinlemesi tavanı makinede ne olursa olsun `b × 1`'e indirir.
3. Tavan bir **zaman** tavanıdır, **iş** tavanı değil: SMT çiftinde harcanan 200 ms ~1.65 CPU'luk iş üretir, 2 değil (§3.4/§3.5) — ms'ler eşittir, arkalarındaki silikon değildir.

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

**Tasarım problemi:** 2 vCPU'muz var. İki busy loop ayrı vCPU'lara oturur ve hiç kavga etmez — weight ise yalnızca kavgada hakemlik yapar. Kavgayı, iki cgroup'u **`cpuset.cpus`** ile CPU 0'a kilitleyerek zorluyoruz (üçüncü controller: `cpuset` = *nerede* koşabilirsin, `cpu.max` = *ne kadar*, `cpu.weight` = *run queue'da kim kazanır*).

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

**Perde A — neden 25/25, neden 25/75 değil?** 25/75 tahmin etmiştik ve yanıldık. Process'ler ayrı vCPU'lara kaçtı — ama quota'dan kaçamadılar: **quota CPU başına değil, cgroup'un toplamına yazılır**. İki CPU'da koşmak 50 ms'lik bütçeyi sadece 2× hızla yakar (25 ms'de biter), sonra *ikisi birden* donar. Ama hiç *aynı* CPU'nun run queue'suna girmedikleri için kavga da çıkmadı — ve **weight yalnızca bir CPU'nun run queue'sundaki kavgada hakemlik yapar**. Kavga yoksa bütçe ilk-gelen-alır usulü tüketilir: eşit bölüşüm.

**Perde B** — aynı 50 ms'lik pasta, ama artık ikisi CPU 0'un run queue'sunda → weight hakem: 100:300 ⇒ tahmin 12.5/37.5, ölçüm 12.7/37.3.

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

[↑ Go back to TOC](#i̇çindekiler)

---

# Bölüm 3 — Rust ile yük üreteci

> **Test makinesi hatırlatması** (detaylar [§1.7](#17-gerçek-bir-makineyi-okumak-t3large)'de): AWS EC2 `t3.large` — **1 physical core × 2 SMT thread = 2 vCPU**, Intel Xeon 8259CL @ 2.50 GHz, Ubuntu, cgroup v2. Bu bölümdeki her sayı bu 2 vCPU'ya görelidir.

Kendi ölçümünü yapan bir yük üreteci: N thread, her biri saniyede yaptığı işi sayar. Bu bölümün üç hedefi var, her biri bir deney: 2 vCPU'ya karşı thread sayısı taraması (§3.4 — **concurrency vs parallelism** ayrımının tanımla değil ölçümle ortaya konduğu yer), aynı taramanın throttle'lı cgroup içinde tekrarı (thread × quota matrisi) ve `std::thread::available_parallelism()` ile gerçeğin karşılaştırması.

## 3.1 VM'de Rust toolchain kurulumu

**rustup** ile kur (Rust projesinin resmî installer'ı), distro paketiyle değil — `apt install cargo` genelde bir yıldan eski bir sürüm verir; rustup güncel stable + kolay güncelleme (`rustup update`) sağlar.

```bash
# Kurulum (normal user ile, sudo gerekmez)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Ortamı mevcut shell'e yükle (yeni login'ler otomatik alır)
. "$HOME/.cargo/env"

# Doğrula
cargo --version
```

Her şey `~/.cargo` ve `~/.rustup` altına iner — sistem dosyalarına dokunmaz, `rustup self uninstall` ile kaldırılır.

rustup şu uyarıyı verebilir: `no default linker ('cc') was found in your PATH`. Rust kodunu kendisi derler ama son adım — binary'nin link'lenmesi — sistemin C toolchain'ini kullanır; minimal cloud image'larda o yoktur. Kur ve toolchain'i uçtan uca test et:

```bash
sudo NEEDRESTART_MODE=a apt update && sudo NEEDRESTART_MODE=a apt install -y build-essential
```

(`NEEDRESTART_MODE=a`: Ubuntu server'da kurulu gelen `needrestart` aracı, kütüphane güncellemelerinden sonra interaktif "hangi servisler restart edilsin?" dialog'u açar; `a` = gerekeni sormadan otomatik restart et.)

```bash
cargo new hello && cd hello && cargo run   # "Hello, world!" basarsa toolchain tamam
```

**Repo düzeni notu:** bu lab'daki burner programları [`burners/`](burners/) altında tek dosyalık, yalnız-std örneklerdir ve doğrudan `rustc -O` ile derlenir — cargo şimdilik hiçbir şey katmaz (dependency yok). Crate gerektiği gün (örn. async deneyleri) cargo geri döner. Cargosuz yolun tek riski: **`-O` artık senin sorumluluğunda** — `rustc` default'ta optimize *etmez* ve optimizasyonsuz benchmark sayıları çöptür.

Yan yana geçen iki bayrağı karıştırma (`-0` — rakam sıfır — diye bir bayrak ise hiç yok):

```bash
rustc -O burners/02_threads.rs -o burners/bin/02_threads
#      ↑ büyük O: Optimize          ↑ küçük o: output — binary'nin adını verir
```

## Deney 3.2 — ilk ölçüm: saat sorunu

Bölüm 2'de hakem `top`'tu; o CPU *doluluğunu* gösterir. Bundan sonra programın kendisi ölçüp *gerçek iş çıktısını* raporlayacak — çünkü throttling altında doluluk "%50" der, ama bunun neye mal olduğunu yalnızca iş hızı söyler. Kod bu repo'da: [`burners/01_baseline.rs`](burners/01_baseline.rs):

```rust
use std::time::{Duration, Instant};

fn main() {
    let secs = 5;
    let start = Instant::now();
    let mut count: u64 = 0;

    while start.elapsed() < Duration::from_secs(secs) {
        count += 1;
    }

    let rate = count as f64 / secs as f64 / 1_000_000.0;
    println!("{count} iterations in {secs} s  ->  {rate:.1} M iter/s");
}
```

Satır satır:

| Kod | Ne yapıyor |
|---|---|
| `Instant::now()` | Kronometreyi başlatır. *Monotonic* saat — duvar saati değişimlerinden (NTP, yaz saati) etkilenmez. |
| `start.elapsed() < Duration::from_secs(secs)` | "5 sn dolana kadar dön." Saat okuması ucuzdur — syscall değil; vDSO üzerinden user space'te okunur. |
| `count += 1` | "İş": bir `u64`'ü olabildiğince hızlı artırmak. |
| `count as f64 / secs / 1e6` | **M iter/s**'e normalize eder — bundan sonraki tüm deneylerin ortak birimi. |

İki şekilde derleyip koş (binary'ler git-ignore'lu `burners/bin/`'e gider):

```bash
mkdir -p burners/bin
rustc    burners/01_baseline.rs -o burners/bin/01_baseline && ./burners/bin/01_baseline   # optimizasyonsuz
rustc -O burners/01_baseline.rs -o burners/bin/01_baseline && ./burners/bin/01_baseline   # optimize
```

Gerçek çıktı (t3.large):

```
131746254 iterations in 5 s  ->  26.3 M iter/s        ← optimizasyonsuz
169633279 iterations in 5 s  ->  33.9 M iter/s        ← optimize (-O)
```

**Ölçüm dersi #1.** Release burada yalnızca ~%29 hızlı — oysa saf hesap döngülerinde fark rutin olarak 10–100× olur. Açıklaması: her iterasyon `start.elapsed()` çağırıyor; döngünün baskın maliyeti optimizer'ın kaldıramayacağı saat okuması. Bu haliyle program "saniyede toplama" değil, "saniyede saat okuması" benchmark'ına daha yakın. Her zaman *döngüdeki en pahalı şeyi* ölçersin — onun ne olduğunu bil. Sonraki sürüm bunu, saate her turda değil her N turda bir bakarak düzeltecek. Kalıcı kural her durumda geçerli: **benchmark sayısı yalnızca optimize build'den (`rustc -O`) sayılır.**

## Deney 3.3 — temiz ölçüm, N thread

Yenilenmiş burner: [`burners/02_threads.rs`](burners/02_threads.rs). 01'e göre farklar: saate her turda değil 1 M iterasyonda bir bakılıyor (döngünün maliyeti artık gerçekten sayma), `std::hint::black_box` optimizer'ın sayma döngüsünü tek toplamaya indirgemesini engelliyor, thread sayısı CLI argümanı olarak geliyor. Kodun tamamı, Rust'a yeni başlayan için açıklamalı:

```rust
use std::env;
use std::time::{Duration, Instant};

const BATCH: u64 = 1_000_000;            // iki saat okuması arasında kaç sayım

// TEK thread'in işi: `secs` saniye boyunca say, toplamı döndür.
fn burn(secs: u64) -> u64 {
    let start = Instant::now();
    let mut count: u64 = 0;
    while start.elapsed() < Duration::from_secs(secs) {   // saat okuması: BATCH'te bir
        for _ in 0..BATCH {
            // black_box = "derleyici, bunu GERÇEKTEN hesaplamak zorundasın".
            // O olmasa -O tüm döngüyü `count += BATCH`'e indirger,
            // hiçbir şey ölçmemiş olurduk.
            count = std::hint::black_box(count + 1);
        }
    }
    count
}

fn main() {
    let secs = 5;

    // İlk CLI argümanı = thread sayısı. `nth(1)` program adını atlar;
    // argüman yoksa ya da sayı değilse 1'e düşülür.
    let threads: usize = env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);

    let start = Instant::now();

    // N özdeş thread başlat. `move`, her closure'a `secs`in kendi kopyasını verir.
    // Her `spawn` bir JoinHandle döndürür — o thread'in sonucunu alma bileti.
    let handles: Vec<_> = (0..threads).map(|_| std::thread::spawn(move || burn(secs))).collect();

    // `join()` thread bitene kadar bekler ve dönüş değerini teslim eder.
    // Tüm biletlerin toplamı = tüm thread'lerin yaptığı toplam iş.
    let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();

    let wall = start.elapsed().as_secs_f64();   // gerçekte geçen duvar saati süresi

    let rate = total as f64 / wall / 1_000_000.0;
    println!("{threads} thread(s): {total} iters in {wall:.2} s  ->  {rate:.0} M iter/s total");
}
```

Çalışma modeli tek cümlede: `main`, her biri 5 saniye boyunca *bağımsız* sayan (paylaşılan veri yok, lock yok) N işçi başlatır; `join` satırında bitiş çizgisi gibi bekler, thread başına toplamları toplar ve *duvar saati* süresine böler — yani basılan hız thread başına hız değil, makinenin toplam throughput'udur.

```bash
mkdir -p burners/bin
rustc -O burners/02_threads.rs -o burners/bin/02_threads
```

```bash
./burners/bin/02_threads 1     # 1 thread; argüman = thread sayısı (default 1)
```

Gerçek çıktı (t3.large), sürüm 1 vs sürüm 2 — aynı makine, aynı "iş":

```
$ ./burners/bin/01_baseline
172688794 iterations in 5 s   ->   34.5 M iter/s          ← v1: her iterasyonda saat okuma
$ ./burners/bin/02_threads 1
1 thread(s): 2885000000 iters in 5.00 s  ->  577 M iter/s ← v2: 1 M iterasyonda bir saat okuma
```

**Ölçüm dersi #1, kapandı.** 17× fark — makine hızlandığı için değil: v1 fiilen saat okumayı benchmark'lıyordu, v2 gerçek saymayı ölçüyor. (Sağlama: 2.5 GHz core'da 577 M toplama/s ≈ iterasyon başına ~4 cycle; `black_box`'ın zorladığı bellek trafiğiyle makul.) Sayıya güvenmeden önce ölçümü düzelt. *(Thread taraması sonuçları: sırada.)*

## Deney 3.4 — thread taraması: parallelism duvarı

Tek binary, tek değişken: thread sayısı. Artı bir özel koşu — tek CPU'ya kilitlenmiş 2 thread — *thread sayısı sabitken* concurrency ile parallelism'i birbirinden ayıran koşu.

### Referans hangi sayı?

Şimdiye kadar iki program göründü ve rolleri farklı:

- **`01_baseline.rs` bir öğretim demosudur, referans değildir.** Görevi *saat sorununu* göstermekti: her iterasyonda saat okuyordu ve bir saat okuması saymanın kendisinden ~17× pahalı olduğu için sayısı (34.5 M iter/s) işi değil, saat okumalarını ölçüyordu (§3.2).
- **`02_threads 1` (yani 1 thread'li `02_threads.rs`) referanstır.** Saat sorunu çözülünce (milyon iterasyonda bir okuma), tek vCPU'daki tek thread **578 M iter/s** üretir — "bir vCPU'nun değeri". **Aşağıdaki her karşılaştırma bu sayıya göredir**, asla 01'e göre değil.

### Araç: `taskset`

`taskset`, bir process'in **CPU affinity**'sini ayarlar: kernel scheduler'ının onu koyabileceği logical CPU kümesi. `taskset -c 0 <komut>`, `<komut>`'u yalnız CPU 0'da koşmaya izinli başlatır — process'in tüm thread'leri kısıtı miras alır; iki aç thread'in tek vCPU'da sırayla dönmekten başka çaresi kalmaz.

```bash
taskset -c 0 burners/bin/02_threads 2     # yalnız CPU 0'a kilitli başlat
taskset -c 0,1 <komut>    # CPU 0 ve 1'e izinli başlat (aralık da olur: -c 0-3)
taskset -cp <pid>         # koşan bir process'in mevcut affinity'sini göster
taskset -cp 0 <pid>       # koşan process'i canlı canlı CPU 0'a kilitle
```

Burada neden lazım: onsuz, scheduler 2 aç thread'i anında 2 vCPU'ya yayar; concurrency ile parallelism birlikte yükselir — ayırt edilemez. Pinleme, concurrency'yi tutarken parallelism'i söker; izole etmek istediğimiz değişken tam olarak bu. Scheduler'ın iki durumda yaptığı, zaman çizgisinde:

```
02_threads 2  (iki CPU da serbest)         taskset -c 0 02_threads 2  (yalnız CPU 0)
CPU 0: AAAAAAAAAAAAAAAA                    CPU 0: AAAA BBBB AAAA BBBB ...
CPU 1: BBBBBBBBBBBBBBBB                    CPU 1: (boş)
→ A ile B gerçekten eşzamanlı             → A ile B sırayla; herhangi bir anda
  (parallelism = 2)                          yalnız biri koşuyor (parallelism = 1)
```

İki durumda da concurrency = 2 (uçuşta iki thread). Parallelism = 2 yalnız soldakinde — ve yalnız soldaki hızlı.

Bölüm 2'deki `cpuset.cpus` ile ilişkisi — aynı kernel yeteneği, iki arayüz:

| | `taskset` | cgroup `cpuset.cpus` |
|---|---|---|
| Kapsam | tek process (+ thread'leri/çocukları) | cgroup'taki her process |
| Yetki | gerekmez (kendi process'lerin) | root (cgroup dosya yazmaları) |
| Zorlayıcılık | tavsiye niteliğinde — process kendi `sched_setaffinity()` çağrısıyla kaçabilir | zorunlu — cgroup duvarı içeriden aşılamaz |
| Tipik kullanım | hızlı deneyler, benchmark | container'lar, Kubernetes static CPU manager |

```bash
rustc -O burners/02_threads.rs -o burners/bin/02_threads
burners/bin/02_threads 1
burners/bin/02_threads 2
taskset -c 0 burners/bin/02_threads 2
burners/bin/02_threads 4
burners/bin/02_threads 8
```

Gerçek çıktı (t3.large — 2 vCPU = **tek** fiziksel core'un 2 SMT thread'i):

```
1 thread(s): 2889000000 iters in 5.00 s  ->  578 M iter/s total
2 thread(s): 4777000000 iters in 5.00 s  ->  955 M iter/s total
2 thread(s): 2886000000 iters in 5.00 s  ->  577 M iter/s total   ← taskset -c 0
4 thread(s): 4648000000 iters in 5.00 s  ->  929 M iter/s total
8 thread(s): 4249000000 iters in 5.01 s  ->  848 M iter/s total
```

| Koşu | Concurrency | Parallelism | M iter/s | `02_threads 1`'e göre |
|---|---|---|---|---|
| `02_threads 1` | 1 | 1 | 578 | **1.00× (referans)** |
| `02_threads 2` | 2 | 2 | **955** | **1.65×** — 2× değil! |
| `taskset -c 0 02_threads 2` | 2 | **1** | **577** | **1.00×** |
| `02_threads 4` | 4 | 2 | 929 | 1.61× |
| `02_threads 8` | 8 | 2 | 848 | 1.47× |

### Koşu koşu

**`02_threads 1` — referans.** Tek thread; bir vCPU dolu, biri boş. Amaç: "bir vCPU'nun değeri" birimini kurmak (578 M iter/s). Diğer her satır bu sayıya göre yargılanır.

**`02_threads 2` — tam parallelism.** İki thread, iki vCPU, kısıt yok — makinenin en iyi hali. Naif beklenti: 2× = 1156. Ölçüm: **955 = 1.65×**. Kayıp %35'in adı SMT: bu iki vCPU, *tek* mutfağın iki sipariş panosu (§1.2); thread'ler core'un execution unit'lerini paylaşıyor. SMT kazancı iş yüküne bağlıdır (genel kural +%20–30; bu toplama döngüsü +%65 aldı) ama asla +%100 olmaz. **"2 vCPU" ≠ "2 core" — işte ölçümü.**

**`taskset -c 0 02_threads 2` — parallelism'siz concurrency.** Kontrol deneyi: aynı iki thread, ama CPU 0'a hapsedilmiş — aynı anda koşmak yerine *sırayla dönüşüyorlar*. Concurrency 2; parallelism 1. Ölçüm: **577 ≈ 1-thread baseline'ın tıpkısı**. Concurrency'yi ikiye katlamak iş hızını kıpırdatmadı. **Hızı parallelism verir; concurrency yalnızca yapı verir** (beklemeleri örtüştürmeye yarar — ama saf hesap döngüsünde bekleyecek bir şey yok).

**`02_threads 4` ve `02_threads 8` — duvarın ötesi.** Donanımın aynı anda koşturabileceğinden çok thread: parallelism 2'de çakılı, concurrency büyüyor. Beklenti: ~955'te düz çizgi. Ölçüm: **929 ve 848 — önce düz, sonra sarkıyor** (8 thread'de zirvenin ~%11 altı).

Kayıp nereden geliyor? Mekaniği takip et:

1. **8 koşulabilir thread ÷ 2 logical CPU = CPU başına ~4 thread sırada.** Herkes her an koşmak istiyor; donanımın oturtabildiği iki.
2. **Scheduler adaleti rotasyonla sağlar.** Linux'un scheduler'ı (CFS, yeni kernel'lerde EEVDF) her thread'e kısa bir zaman dilimi verir, sonra değiştirir: A'yı CPU'dan kaldır, B'yi oturt, birkaç milisaniye sonra B'yi kaldır, C'yi oturt… Her thread ilerler, hiçbiri hızlı ilerlemez.
3. **Her değişimin — *context switch* — iki bedeli vardır.** **Doğrudan** bedel küçük ve görünürdür: A'nın register'larını kaydet, B'ninkileri yükle, scheduler'ın defter işlerini koştur — mikrosaniyeler mertebesinde. **Dolaylı** bedel ise sinsi olandır: A koşarken CPU'nun L1/L2 cache'leri *A'nın* verisiyle dolmuştu. B soğuk bir cache'e gelir ve cache yeniden ısınana dek memory beklemesinde takılır — hiç iş üretmeyen cycle'lar; üstelik hiçbir "context switch süresi" istatistiğinde görünmezler.
4. **Sürtünme benzetmesi.** İtilen kütle (toplam iş) değişmedi; yalnız temas noktası sayısı (switch'ler) arttı — ve her temas bir miktar enerjiyi ısıya çevirir. 955 → 848 o ısıdır: **%11 sürtünme kaybı**.

**Duvarın ötesinde thread'ler yardım etmeyi bırakmakla kalmaz — maliyet çıkarmaya başlar.**

−%11'imizi *en iyi ihtimal* yapan incelik: bu iterasyonlar tamamen bağımsız — paylaşılan veri yok, lock yok, working set minicik. Gerçek uygulamalar durum paylaşır; thread'leri birbirinin cache line'larını söker, lock queue'larında bekleşir — oversubscription sürtünmesi bu temiz döngünün gösterdiğinden tipik olarak çok daha ağırdır.

### Dersler

1. **SMT, CPU sayısını şişirir**: `nproc` okuyup "N vCPU = N core'luk iş" varsayan kapasite planı, tasarımı gereği fazla vaat eder.
2. **Thread sayısı hız değil yapı beyanıdır**; hızın tavanını eldeki paralel donanım çizer.
3. **Oversubscription'ın bedeli vardır** — ve Part 3'ün cgroup deneylerinde bu bedelin dişleri çıkacak: kısıtlı quota'ya karşı 8 thread bütçeyi 8× hızlı yakar, sonra hep birlikte donar.

## Deney 3.5 — thread × quota matrisi

Bölüm 2'de oyuncak bir döngüyü kısıp `top`'a bakmıştık; şimdi *ölçüm aletinin kendisini* kısıp gerçek sayılar okuyoruz. İki değişken, tek ızgara: thread sayısı (1 / 2 / 8) × `cpu.max` (limitsiz / 1 vCPU / 0.5 vCPU) — dokuz hücre.

### Yöntem: kafesteki shell

Bir benchmark koşusu 5 saniye sürüyor — PID'ini yakalayıp uçuş sırasında cgroup'a taşımak için çok kısa. Çözüm bir miras kuralı: **process, parent'ının cgroup'una doğar.** Shell'i kafese bir kez koyarız; ondan sonra başlattığı her komut ilk instruction'ından itibaren içeride başlar.

Deneyin tamamı **tek terminalde** koşar: shell bir kez kafese girince quota değişimleri (`sudo tee`) ve `cpu.stat` okumaları da kafes içinde koşar — anlık işlerdir, ölçümü bozmazlar.

```bash
# Hücreyi yarat, sonra BU shell'i kafese koy ($$ = shell'in kendi PID'i)
sudo mkdir /sys/fs/cgroup/lab
echo $$ | sudo tee /sys/fs/cgroup/lab/cgroup.procs
cat /proc/$$/cgroup                    # 0::/lab demeli

# Her kolon için: quota'yı kur, üç thread sayısını koş,
# HER koşudan sonra kernel şahidini oku.
echo "max 100000"    | sudo tee /sys/fs/cgroup/lab/cpu.max   # kolon 1: limitsiz
# (kolon 2: "100000 100000" = 1 vCPU · kolon 3: "50000 100000" = 0.5 vCPU)

burners/bin/02_threads 1
grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
burners/bin/02_threads 2
grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
burners/bin/02_threads 8
grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat

# Temizlik — tuzağıyla birlikte: shell'in hâlâ içerideyken rmdir
# "Device or resource busy" ile reddedilir. Önce kendini tahliye et
# (root cgroup'a geri taşın), sonra hücreyi sil:
echo $$ | sudo tee /sys/fs/cgroup/cgroup.procs
cat /proc/$$/cgroup                    # 0::/ — yeniden özgürsün
sudo rmdir /sys/fs/cgroup/lab
```

`cpu.stat` neden kolon başına değil her koşudan sonra okunur? Sayaçlar **kümülatiftir**; suçu doğru koşuya ancak koşu-başına farklar yıkar. `nr_periods` geçen quota penceresi sayısıdır, `nr_throttled` grubun bütçesini bitirdiği için dondurulduğu pencere sayısı — Kubernetes'in Prometheus'a `container_cpu_cfs_throttled_periods_total` diye ihraç ettiği, her "pod'um yavaş" soruşturmasının bir numaralı metriği olan sayacın ta kendisi.

### Sonuçlar

Tam koşu (t3.large), dokuz hücrenin tamamı, her koşudan sonra şahit okumasıyla:

```
$ echo "max 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max        # ── kolon 1: limitsiz
$ burners/bin/02_threads 1
1 thread(s): 2906000000 iters in 5.00 s  ->  581 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 324
nr_throttled 249
$ burners/bin/02_threads 2
2 thread(s): 4289000000 iters in 5.00 s  ->  858 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 324
nr_throttled 249
$ burners/bin/02_threads 8
8 thread(s): 4280000000 iters in 5.01 s  ->  854 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 324
nr_throttled 249

$ echo "100000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max     # ── kolon 2: 1 vCPU
$ burners/bin/02_threads 1
1 thread(s): 2872000000 iters in 5.00 s  ->  574 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 379
nr_throttled 249
$ burners/bin/02_threads 2
2 thread(s): 2454000000 iters in 5.01 s  ->  489 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 433
nr_throttled 296
$ burners/bin/02_threads 8
8 thread(s): 2457000000 iters in 5.01 s  ->  490 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 488
nr_throttled 344

$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max      # ── kolon 3: 0.5 vCPU
$ burners/bin/02_threads 1
1 thread(s): 1442000000 iters in 5.00 s  ->  288 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 548
nr_throttled 394
$ burners/bin/02_threads 2
2 thread(s): 1207000000 iters in 5.01 s  ->  241 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 603
nr_throttled 444
$ burners/bin/02_threads 8
8 thread(s): 1258000000 iters in 5.04 s  ->  250 M iter/s total
$ grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
nr_periods 660
nr_throttled 495
```

Throughput (M iter/s):

| | limitsiz | 1 vCPU | 0.5 vCPU |
|---|---|---|---|
| **1 thread** | 581 | 574 | 288 |
| **2 thread** | 858 | 489 | 241 |
| **8 thread** | 854 | 490 | 250 |

**Kernel şahidi deltaları nasıl hesaplanır.** `cpu.stat` sayaçları kümülatiftir — cgroup yaşadıkça asla sıfırlanmaz. Yani tek bir okuma *bu* koşu hakkında hiçbir şey söylemez; bir koşuya ait olan, **koşudan sonraki okuma ile öncesindeki okuma arasındaki farktır** (öncesi = bir önceki koşunun okuması). Yukarıdaki ham dökümden adım adım:

| Koşu | önce (periods / throttled) | sonra | Δ`nr_periods` | Δ`nr_throttled` |
|---|---|---|---|---|
| 1 vCPU, 1 thread | 324 / 249 | 379 / 249 | 55 | **0** |
| 1 vCPU, 2 thread | 379 / 249 | 433 / 296 | 54 | 47 |
| 1 vCPU, 8 thread | 433 / 296 | 488 / 344 | 55 | 48 |
| 0.5 vCPU, 1 thread | 488 / 344 | 548 / 394 | 60 | 50 |
| 0.5 vCPU, 2 thread | 548 / 394 | 603 / 444 | 55 | 50 |
| 0.5 vCPU, 8 thread | 603 / 444 | 660 / 495 | 57 | 51 |

(Limitsiz kolon 324 / 249'da donuk kaldı — bunlar aynı cgroup'taki daha önceki denemelerin kalıntısı; limit yokken muhasebe hiç çalışmaz. Koşu başına ~55 pencere de tesadüf değil: 5 sn koşu ÷ 100 ms period ≈ 50 pencere + komutlar arası birkaç pencerelik shell hareketi.)

Kernel şahidi (koşu başına Δ`nr_throttled` / Δ`nr_periods`):

| | limitsiz | 1 vCPU | 0.5 vCPU |
|---|---|---|---|
| **1 thread** | 0 / 0 | **0** / 55 | 50 / 60 |
| **2 thread** | 0 / 0 | 47 / 54 | 50 / 55 |
| **8 thread** | 0 / 0 | 48 / 55 | 51 / 57 |

Hücre okuma örneği: "47 / 54" = bu koşu sırasında geçen 54 quota penceresinin 47'sinde grup donduruldu — pencerelerin %87'sinde throttle.

*(Yan not: limitsiz kolonda `nr_periods` hiç ilerlemedi — bandwidth muhasebesi yalnız bir limit varken çalışır. Limitsiz 2-thread hücresi 858 ölçtü, §3.4'te 955'ti: t3'ün CPU credit'leri erimeye başlamıştı; `%st`'yi izle.)*

### Izgaranın öğrettikleri

**Satır 1 — quota tek thread'e karşı dürüsttür.** 581 → 574 → 288: 1 vCPU'da ölçülebilir maliyet yok, yarım vCPU'da tam yarısı. Ve ince mücevher: 1 vCPU'da Δ`nr_throttled` = **0** — tek bir thread fiziksel olarak birden fazla CPU işgal edemez, bütçe tavanına hiç dokunmaz. Kubernetes çevirisi: **uygulamanın kullanabileceğinden büyük limit etkisizdir** — ne fayda ne zarar.

**Kolon 2 — bomba: 1 vCPU quota altında 2 thread, 1 thread'den *az* üretir (489 < 574).** Aynı bütçe, daha çok işçi, daha az iş. Mekanik: iki thread 100 ms'lik bütçeyi iki SMT kardeşinde 2× hızla yakar — ~50 ms'de biter, kalan süre donuk — ama SMT çifti koşarken yalnız ~1.65× üretir. **SMT çiftinde harcanan bir quota-milisaniyesi, tam bir CPU'da harcanandan daha az iş üretir**; donma/uyanma döngüsü de kendi vergisini ekler. Kernel doğruluyor: pencerelerin ~%87'sinde throttle. Prod bilmecesi "`limit: 1` verdik — uygulama neden tek thread'den yavaş?" işte bu hücreyle cevaplanır.

**Kolon 3 — acı hücre.** Tek thread temiz yarıyı alır (288); 2 ve 8 thread ~%15 ek vergi öder (241–250) ve throttle oranını ~%90'a yapıştırır. Ama throughput *hafif* semptomdur — dar quota altındaki çok thread'in asıl hasarı donma deseni; onu §3.6 doğrudan ölçecek.

**Izgaranın kanıtladığı boyutlandırma kuralı:** CPU limiti altında thread sayısını `nproc`'un iddiasına değil, **limite** eşle (⌈limit⌉ thread). Dar quota altındaki fazla thread saf kayıptır: aynı ya da daha az throughput, azami throttling.

## Deney 3.6 — stall'lar: matrisin göremediği acı

Deney 3.5 "throughput yarılandı" ile bitti — nahoş ama katlanılır. Matrisin *gösteremediği* şey: dar quota altında iş yavaş-ve-düzgün akmaz. **Patlamalar halinde akar, aralarında ölü donmalar vardır.** Bir server için o donmalar, queue'da kıpırdamadan bekleyen isteklerdir.

**Bunu neden ne `top` ne throughput gösterir: ikisi de ortalamadır.** `top` CPU kullanımını yenileme aralığı üzerinden örnekler; benchmark'ımız toplam işi 5 saniyeye böler. 100 ms'lik bir donma iki ortalamanın da içinde kaybolur — "%50 CPU", *her şey yarı hızda akıyor* da olabilir, *yarı zaman tam hız, yarı zaman taş kesilmiş* de. Throughput için ikisi aynıdır; latency için apayrı dünyalardır. İkisini ayırt edebilen tek gözlemci **process'in içinde** oturur ve kendi ilerleyişini zaman damgalar. Burner'ın stall'unu kendisinin ölçmesi bu yüzden şarttır.

### Araç: `03_stalls.rs`

[`burners/03_stalls.rs`](burners/03_stalls.rs) = `02_threads` + tek fikir: her thread, **ardışık iki batch bitişi arasındaki en uzun boşluğu** aklında tutar. Kalbi, açıklamalı:

```rust
fn burn(secs: u64) -> (u64, Duration) {          // artık İKİ şey döndürür:
    let start = Instant::now();                  //   (toplam sayım, en kötü boşluk)
    let mut count: u64 = 0;
    let mut max_stall = Duration::ZERO;          // görülen en kötü boşluk: 0'dan başlar
    let mut last = Instant::now();               // bir ÖNCEKİ batch bitişinin zaman damgası

    while start.elapsed() < Duration::from_secs(secs) {
        for _ in 0..BATCH {                      // batch: 1 M sayım
            count = std::hint::black_box(count + 1);
        }
        let now = Instant::now();                // bu batch az önce bitti
        let gap = now - last;                    // önceki bitişten bu yana geçen süre —
        if gap > max_stall {                     //   iş süresi VE aradaki her kesinti
            max_stall = gap;                     // yalnız rekor sahibini tut
        }
        last = now;                              // bu bitiş yeni referans olur
    }
    (count, max_stall)
}
```

Ölçüm adım adım şöyle çalışır: thread, tur atan bir koşucu gibi batch'ten batch'e yaşar. `last` her zaman önceki turun bitiş saatini tutar; her yeni bitişte `gap` = turun gerçek süresi hesaplanır — CPU işi *artı* onu kesen her şey (quota donması, run queue'da bekleme). Limitsiz ve boş bir CPU'da her tur ~2 ms sürer, `max_stall` ~2 ms'de kalır. Kernel grubu tur ortasında 50 ms dondurduysa o turun `gap`'i ~52 ms'e fırlar ve `max_stall` bunu kayda geçirir. `main`'de her thread kendi maksimumunu döndürür, thread'ler arasındaki en kötüsü basılır — *herhangi bir* thread'in en uzun süre kıpırdamadan durduğu süre.

Dürüst bir sınır: yalnız *maksimumu* tutuyoruz; çıktı en kötü anın ne kadar kötü olduğunu söyler, kötü anların ne sıklıkta olduğunu değil. (Gerçek bir latency benchmark'ı histogram tutup p50/p99 basardı — o inceltme tokio bölümüne kalsın.)

### Koşular

Kafes kurulumu §3.5 ile aynı (shell cgroup'ta, tek terminal). Altı koşu: iki thread sayısı (1 / 8) × üç rejim — limitsiz, 0.5 vCPU standart 100 ms period, 0.5 vCPU 10 ms period (aynı oran, 10× kısa pencere).

```bash
rustc -O burners/03_stalls.rs -o burners/bin/03_stalls

burners/bin/03_stalls 1                                      # ── limitsiz
burners/bin/03_stalls 8
echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max    # ── 0.5 vCPU, 100 ms period
burners/bin/03_stalls 1
burners/bin/03_stalls 8
echo "5000 10000" | sudo tee /sys/fs/cgroup/lab/cpu.max      # ── 0.5 vCPU, 10 ms period
burners/bin/03_stalls 1
burners/bin/03_stalls 8
```

Tam koşu (t3.large):

```
$ cat /sys/fs/cgroup/lab/cpu.max
max 100000
$ burners/bin/03_stalls 1
1 thread(s): 578 M iter/s total, worst stall: 2.1 ms
$ burners/bin/03_stalls 8
8 thread(s): 909 M iter/s total, worst stall: 30.8 ms

$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max
$ burners/bin/03_stalls 1
1 thread(s): 289 M iter/s total, worst stall: 53.3 ms
$ burners/bin/03_stalls 8
8 thread(s): 237 M iter/s total, worst stall: 115.1 ms

$ echo "5000 10000" | sudo tee /sys/fs/cgroup/lab/cpu.max
$ burners/bin/03_stalls 1
1 thread(s): 285 M iter/s total, worst stall: 7.7 ms
$ burners/bin/03_stalls 8
8 thread(s): 272 M iter/s total, worst stall: 135.9 ms
```

### Sonuçlar

| Koşu | M iter/s | Worst stall |
|---|---|---|
| limitsiz, 1 thread | 578 | 2.1 ms |
| limitsiz, 8 thread | 909 | **30.8 ms** |
| 0.5 vCPU · 100 ms period, 1 thread | 289 | 53.3 ms |
| 0.5 vCPU · 100 ms period, 8 thread | 237 | 115.1 ms |
| 0.5 vCPU · 10 ms period, 1 thread | 285 | **7.7 ms** |
| 0.5 vCPU · 10 ms period, 8 thread | 272 | **135.9 ms** |

### Anatomi: beklemenin iki kaynağı

Altı sayı, "stall"un tek bir şey olmadığını görene kadar düzensiz görünür. Koşmayan bir thread, **iki bağımsız mekanizmadan** birini bekliyordur:

- **Kaynak A — run queue beklemesi.** CPU'dan çok runnable thread var: scheduler onları rotasyonla döndürür, senin thread'in *başkaları koşarken* bekler. Berber dükkânı düşün: iki koltuk (vCPU), sekiz müşteri (thread) — ziyaretinin çoğu koltukta değil, bekleme sırasında geçer.
- **Kaynak B — quota donması.** cgroup'un bu penceredeki bütçesi bitti; kernel bir sonraki pencere açılana dek *herkesi* durdurur. Dükkânın kepengi iner — sıra ne kadar kısa olursa olsun kimseye hizmet yok.

İkisi bağımsızdır: A'ya çekişme lazım (çok thread), B'ye limit lazım (quota). Tablomuzdaki her koşu birini, öbürünü ya da ikisini birden açıyor:

| Koşu | Stall | Teşhis |
|---|---|---|
| limitsiz, 1 thread | 2.1 ms | ikisi de yok — batch'in kendi süresi |
| limitsiz, 8 thread | 30.8 ms | saf **A**: koltuk başına 4 thread, ~3–4 tur bekleme |
| 0.5 vCPU · 100 ms, 1 thread | 53.3 ms | saf **B**: 50 ms kepenk (period − quota) + ~3 ms batch |
| 0.5 vCPU · 100 ms, 8 thread | 115.1 ms | **A + B**: kepenk kalkıyor — ama sıra hâlâ sende değil; sonraki pencerede yine donma |
| 0.5 vCPU · 10 ms, 1 thread | 7.7 ms | saf B, küçültülmüş: 5 ms kepenk + batch |
| 0.5 vCPU · 10 ms, 8 thread | 135.9 ms | **A × B, en kötü hal**: her pencere 5 ms'lik bir kırıntı için açılıyor, sekiz aç thread onu kapışıyor — şanssız thread üst üste *pencere serileri* boyunca hiç sıra alamayabilir |

Buradan düşen teşhis kuralı: **1-thread satırlarında yalnız B var — stall formülle öngörülebilir ve period'la küçülür. 8-thread satırlarında A devreye girer ve B ile birleşir — stall artık queue'nun uzunluğuna aittir ve hiçbir period ayarı bir queue'yu kısaltamaz.** Aynı düğmenin (10 ms period) bir satırı iyileştirip (53 → 7.7) öbürüne hiçbir şey yapamamasının (115 → 136) sebebi budur.

### Öğrettikleri

1. **Oran hızı belirler; period belirlemez.** 289 vs 285, 237 vs 272 — period 10× değişti, throughput kımıldamadı. Ortalama hız = quota ÷ period, nokta.
2. **Tek thread için period, acı düğmesidir.** Worst stall ≈ (period − quota) + bir batch: 53.3 ms ≈ 50 ms donma + ~3 ms iş; period'u 10 ms'e indir, stall 7.7 ms'e çöker — aynı throughput, 7× yumuşak tail latency'si. kubelet'in `cpuCFSQuotaPeriod` düğmesinin ta kendisi.
3. **Oversubscription, hiç limit yokken bile bir latency makinesidir.** 8 thread, limitsiz: 30.8 ms worst stall — kimse dondurulmadı; bu, 2 vCPU'da 8 thread'in saf sıra beklemesi (§3.4'ün sürtünmesi, latency tarafından görünüşü).
4. **Dar quota altında ise period ilacını da etkisiz bırakır.** Kısa period'un 8 thread'i de kurtaracağını tahmin etmiştik — kurtarmadı (115 → 136 ms). 10 ms'lik pencerede bütçe, 8 aç thread'in paylaştığı 5 ms'lik bir kırıntıdır; şanssız thread *iki* queue'yu birden bekler — donmaları *ve* kendi turunu — üst üste pencereler boyunca. Queue baskınsa çare period ayarı değil, **thread azaltmaktır**. (Bu lab'ın 2. yanlış tahmini; ikisinde de düzeltme, tahminden çok şey öğretti.)

Kubernetes finali: "p99 patladı ama CPU %50 görünüyor"un anatomisi budur — pod yavaş değil, *kekeliyor*. Teşhis: tırmanan `container_cpu_cfs_throttled_periods_total`. Tedavi, sırasıyla: thread sayısını limite eşle, sonra period'u düşün.

## Deney 3.7 — "kaç CPU var?" sorusuna kim dürüst cevap verir

Her runtime, thread pool'unu sisteme "kaç CPU'm var?" diye sorarak boyutlandırır — ve Bölüm 1, container içinde bu cevabın tuzak olabileceği uyarısını ekmişti. Kimin yalan söylediğini ölçme vakti. Üç bilgi katmanı var ve her cevaplayıcı farklı bir alt kümesini okuyabilir:

1. **Topoloji** — makinede kaç logical CPU var (`/proc`, sysfs).
2. **Affinity** — bu process hangi CPU'larda *koşabilir* (`sched_getaffinity`; `taskset`/`cpuset` belirler).
3. **cgroup quota** — process ne kadar CPU *zamanı* harcayabilir (`cpu.max`; Kubernetes limits belirler).

Araç altı satır — [`burners/04_nproc.rs`](burners/04_nproc.rs), Rust std'nin resmi cevabını basar:

```rust
use std::thread;

fn main() {
    match thread::available_parallelism() {
        Ok(n) => println!("available_parallelism: {n}"),
        Err(e) => println!("error: {e}"),
    }
}
```

Dört senaryo, `nproc` ile Rust cevabı yan yana (kafes düzeni §3.5'teki gibi):

```
$ nproc                                       # ── çıplak: cgroup yok, pinleme yok
2
$ burners/bin/04_nproc
available_parallelism: 2

$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max    # ── kafeste, quota 0.5 vCPU
$ nproc
2
$ burners/bin/04_nproc
available_parallelism: 1

$ echo "150000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max   # ── kafeste, quota 1.5 vCPU
$ nproc
2
$ burners/bin/04_nproc
available_parallelism: 1

$ echo "max 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max      # ── quota yok, CPU 0'a pinli
$ taskset -c 0 nproc
1
$ taskset -c 0 burners/bin/04_nproc
available_parallelism: 1
```

### Sonuçlar

| Senaryo | `nproc` | `available_parallelism()` |
|---|---|---|
| çıplak | 2 | 2 |
| quota 0.5 vCPU | **2** | **1** |
| quota 1.5 vCPU | **2** | **1** |
| CPU 0'a pinli (`taskset`) | 1 | 1 |

### Öğrettikleri

1. **`nproc` affinity okur, quota asla.** 0.5 vCPU limit altında hâlâ 2 der — 64 vCPU'luk node'daki pod'a "64 CPU'n var" diyen katman budur. `nproc` ile boyutlanan her script ve program bu körlüğü miras alır.
2. **Modern Rust std quota'yı da okur — "yalan söyler" anlatısı Rust için eskimiş.** ~1.61'den beri `available_parallelism()`, affinity'nin yanında `cpu.max`'a da bakar: 0.5 vCPU altında 1 cevabını verir. Tahminimiz "yanıltır" demişti — bu lab'ın 3. yanlış tahmini ve en mutlusu: ekosistem bu dersi çoktan almış.
3. **1.5 vCPU sürprizi: Rust *aşağı* yuvarlar.** quota/period = 1.5 → cevap 1 (tabanı 1). Bilinçli muhafazakârlık: 1.5 CPU'luk bütçede iki işçi ikisi birden throttle yer; tek işçi temiz akar, 0.5'lik hak boşta kalır. Utilization değil latency kayırılmış.
4. **Tuzak yalnız bazı dillerde öldü.** `nproc` tabanlı script'ler, düz C, (automaxprocs'suz) Go `GOMAXPROCS`'u, eski JVM'ler hâlâ node sayısını görür — "`limit: 2` altında 64 worker" kazası karışık dilli filolarda gerçekliğini koruyor. tokio'nun ne yaptığını Bölüm 4 *ölçecek*, varsaymayacak.

[↑ Go back to TOC](#i̇çindekiler)

# Bölüm 4 — Async Rust: tokio ve vCPU *(devam ediyor)*

Async bölümü, gerçek bir teslimatla: Bölüm 3'ün burner'ları *sync* dünyayı ölçtü; burada async karşılıklarını inşa ediyoruz — **tokio tabanlı bir RESP yük üreteci** (Redis protokolü konuşan bir client: sabit oranda pipeline'lı SET/GET, p50/p99 latency raporuyla). Async'in ekmeğini gerçekten kazandığı yer network IO'dur; araç ile ders burada çakışır. Yol boyunca: tokio'nun runtime modeli (çok sayıda hafif task taşıyan az sayıda OS worker thread'i), limitli bir cgroup içinde kaç worker açtığı — varsayımla değil ölçümle — ve aynı quota'lar altında CPU-bound vs IO-bound task'lar. Cargo burada geri döner (tokio dış bir crate).

## 4.1 Cargo'nun dönüşü — proje kurulumu

Bölüm 1–3 std dışında hiçbir şeye ihtiyaç duymadı, düz `rustc -O` yetti. tokio dış bir crate — cargo'nun var oluş sebebi tam bu: dependency'yi (ve onun dependency'lerini) crates.io'dan indirir, derler, cache'ler.

```bash
cargo new tokioburn && cd tokioburn
```

`Cargo.toml`'da `[dependencies]` altına:

```toml
tokio = { version = "1", features = ["full"] }
```

(`features = ["full"]` tokio'nun tüm bileşenlerini açar — runtime, net, time. Production build'leri seçici davranır; lab için full pratik.) İlk `cargo build --release` tokio derlenirken 1-2 dakika sürer; sonrakiler saniyeler. Proje bu repo'da: [`tokioburn/`](tokioburn/).

## 4.2 tokio nasıl zamanlar — cooperative, `.await`, task queue'lar

Önce teori — async Rust'ta her şeyin asılı durduğu beş yapı taşı.

### Preemptive vs cooperative

**Kernel scheduler'ı preemptive'dir**: donanımdaki bir timer birkaç ms'de bir interrupt atar; thread ne yapıyor olursa olsun kernel onu zorla durdurur, register'larını kaydeder, başka thread'i oturtur. Thread'e sorulmaz. Bölüm 3'te 8 aç thread'in 2 vCPU'yu paylaşabilmesi bundandı — sonsuz döngü bile komşularını açlıktan öldüremez; kernel CPU'yu düzenli olarak geri alır.

**tokio scheduler'ı cooperative'dir**: runtime, koşan bir task'ı kesemez. Task, worker'ı ancak kendi kodundaki bir teslim noktasına gelince bırakır — o nokta da `.await`'tir. Teslim noktasına hiç gelmeyen task, worker'ı sonsuza dek tutar.

> Kernel scheduler'ı: polis — seni zorla kenara çeker. tokio scheduler'ı: centilmenlik anlaşması — yolu kendin vermelisin.

### `.await` gerçekte ne yapar

Bir `async fn` çağrılınca çalışmaz; bir **Future** üretir — duraklatılabilir bir iş tarifi. `.await` şu demektir: *"Bu sonucu istiyorum; hazır değilse worker'ı serbest bırak, hazır olunca beni buradan devam ettir."*

```rust
let n = socket.read(&mut buf).await;
```

Mekanik: veri henüz yoksa task *o satırda* askıya alınır — konumu ve canlı değişkenleri saklanır (derleyici fonksiyonu bir state machine'e çevirir). Worker anında başka bir task alır. Veri gelince tokio task'ı hazır işaretler; bir worker onu tam kaldığı yerden sürdürür. `.await` hem bekleme noktası hem **worker'ın geri teslim edildiği kapıdır** — cooperative'deki "cooperate" tam burasıdır.

Karanlık taraf doğrudan bundan çıkar: içinde **hiç** `.await` olmayan döngünün — `loop { count += 1 }` — kapısı yoktur. Task çalıştırmak, worker thread'i için düz bir fonksiyon çağrısıdır; fonksiyon `.await`'e hiç gelmiyorsa worker onu durmaksızın işler. tokio kernel değil kütüphanedir: araya girecek timer interrupt'ı yoktur. (Kernel, worker *thread'ini* elbette preempt eder — ama başka process'ler lehine; tokio'nun diğer *task'ları* queue'larda beklemeye devam eder.)

### Task queue'lar ve work-stealing

Önce task'ın doğuşu: `tokio::spawn(async { ... })`, `thread::spawn`'un task dünyasındaki kardeşidir — ama OS thread'i açmaz. Runtime'ın queue'larına hafif bir task (birkaç yüz byte) bırakır ve bir `JoinHandle` döndürür; sırası gelince bir worker onu koşturur.

Koşmaya hazır task'lar nerede bekler? Runtime'ın **task queue'larında** — kernel'in run queue'sunun tokio içindeki karşılığı; şu farkla: sıradakiler thread değil task, yöneten kernel değil tokio:

```
                 ┌──────────────────────────────┐
   tokio::spawn →│  global queue (giriş kapısı) │
                 └──────────────┬───────────────┘
                                ↓ dağıtılır
        worker 0'ın local queue'su      worker 1'in local queue'su
        [task C] [task D]               [task E]
              ↓                                ↓
        worker 0: task A koşuyor        worker 1: task B koşuyor
```

- Her worker'ın kendi **local queue**'su vardır; yeni spawn edilen ve az önce uyanan (timer'ı dolan, IO'su hazır olan) task'lar bu queue'lara düşer.
- Worker'ın elindeki task `.await`'te askıya alınınca worker sıradaki hazır task'ı **kendi local queue'sundan** alır.
- Kendi queue'su boşsa **başka worker'ınkinden çalar** — *work-stealing*, tokio'nun yük dengeleyicisi: birinin birikmişi varken kimse boş durmaz.

### Latency sensörü: heartbeat task

Önümüzdeki deneylerin, zamanlama gecikmesini *içeriden* hisseden bir alete ihtiyacı var. **Heartbeat task** tam olarak şu kadardır:

```rust
loop {
    sleep(Duration::from_millis(100)).await;   // 100 ms sonra uyandırılmayı İSTE
    // uyandığında: saat GERÇEKTE kaç — ne kadar geç kaldım?
}
```

Her şey worker'larda olur — task'lar başka hiçbir yerde çalışmaz. İncelik şurada: `sleep(...).await` sırasında task worker'da **oturmaz**; askıdadır, kimseyi işgal etmez (timer, tokio'nun kendi ajandasında işler). 100 ms dolunca tokio task'ı hazır işaretler, task bir task queue'ya girer — ve bir worker bekler. **Ölçüm o bekleyiştir**: planlanan uyanma T, gerçekleşen koşma T+Δ. Worker'lar boşken Δ ≈ 0; worker'lar `.await`'siz task'larca rehin alınmışken Δ = queue'da geçen süre. Saati, task'ın kendisi okur — worker üstünde, nihayet koştuğu anda.

### "Don't block the event loop"

Her async runtime'ın (Node.js, Python asyncio, tokio) bir numaralı kuralı — ve yukarıdaki üç bölümden doğrudan çıkar: sıradan bir task'ın içindeki CPU-ağır ya da sync-bloke eden kod, worker'ı teslim noktasız işgal eder; o worker'ın queue'sundaki her task bekler. tokio'nun rehberi: iki `.await` arasında bir task kabaca **10–100 µs'den uzun** çalışmamalı. Klasik production belirtisi: bir endpoint async handler içinde ağır parse/sıkıştırma (ya da sync dosya okuması) yapar — ve sunucudaki *tüm* bağlantılar aynı anda takılır.

Kaçış kapısı `tokio::task::spawn_blocking(closure)`: closure'ı **ayrı bir blocking thread havuzuna** taşır (gerçek OS thread'leri, ihtiyaç halinde açılır — default'ta 512'ye kadar) — orayı *kernel'in preemptive* scheduler'ı yönetir; async worker'lar ise `.await` eden task'lar için boş kalır. Kısacası: CPU-bound iş, cooperative dünyadan bunun için inşa edilmiş preemptive dünyaya iade edilir. Yukarıdaki korku hikâyesinde suçun yeri de not edilsin: asla `tokio::spawn`'un kendisinde değil — `.await`'siz CPU işini ona teslim etmekte.

```
yanlış:  2 worker ← 2 CPU-bound task işgal etti  → async dünya kilitli
doğru:   2 worker ← boş: heartbeat'ler, IO, timer'lar akıyor
         + blocking havuzu ← CPU işi burada, kernel adilce preempt ediyor
```

Bundan sonraki deneyler, yukarıdaki her iddiayı sayılara çevirecek.

## Deney 4.3 — tokio kaç worker açar?

tokio'nun multi-thread runtime'ı, çok sayıda hafif task'ı az sayıda OS **worker thread**'i üstünde taşır. O havuzun boyutlandırması, Deney 3.7'nin pratiğe döküldüğü yer: tokio `available_parallelism()`'i mi izliyor — yani cgroup quota'larını görüyor mu — yoksa makineyi mi okuyor? Varsayımla değil, ölçümle.

Sonda ([`tokioburn/src/main.rs`](tokioburn/src/main.rs)):

```rust
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
        tokio::time::sleep(Duration::from_secs(15)).await;
    });
}
```

Yeni kavramlar, satır satır:

| Kod | Ne yapıyor |
|---|---|
| `Builder::new_multi_thread()...build()` | tokio **runtime**'ını kurar: worker thread havuzu + task scheduler'ı. `#[tokio::main]` makrosu bunu görünmez yapar; incelemek için açık kurduk. Worker sayısı vermedik — default'un seçimi *deneyin kendisi*. |
| `rt.metrics().num_workers()` | Runtime'a kaç worker thread açtığını sorar — process içindeki şahit. |
| `rt.block_on(async { ... })` | Sync dünya (`main`) ile async dünya arasındaki kapı: runtime'a ilk task'ını verir ve o task bitene kadar `main`'i bloklar. |
| `tokio::time::sleep(...).await` | tokio'nun uykusu. `.await` = "ben beklerken worker'ı bırak — başka task koşabilsin." (`thread::sleep` worker'ı rehin alırdı; o fark Deney 4.3'ün konusu.) Burada tek işi process'i 15 sn hayatta tutmak — dışarıdan incelenebilsin diye. |

Koşular — çıplak, sonra cgroup içinde 0.5 ve 1.5 vCPU:

```bash
cargo build --release

# ── 1: çıplak ──
./target/release/tokioburn
# program uyurken İKİNCİ terminalden, bastığı PID ile:
ps -T -p <PID>                  # process'in tüm thread'leri, isimleriyle (dış şahit)

# ── 2: kafeste, quota 0.5 vCPU ──
sudo mkdir /sys/fs/cgroup/lab
echo $$ | sudo tee /sys/fs/cgroup/lab/cgroup.procs
echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max
./target/release/tokioburn

# ── 3: kafeste, quota 1.5 vCPU ──
echo "150000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max
./target/release/tokioburn

# ── TEMİZLİK ──
echo $$ | sudo tee /sys/fs/cgroup/cgroup.procs
sudo rmdir /sys/fs/cgroup/lab
```

Gerçek çıktı (t3.large, çıplak):

```
$ ./target/release/tokioburn
available_parallelism: Ok(2)
tokio workers: 2  (PID: 16590)

$ ps -T -p 16590
    PID    SPID TTY          TIME CMD
  16590   16590 pts/1    00:00:00 tokioburn          ← main thread (block_on'da park halinde)
  16590   16591 pts/1    00:00:00 tokio-rt-worker    ← worker 1
  16590   16592 pts/1    00:00:00 tokio-rt-worker    ← worker 2
```

Bakmaya değer iki ayrıntı: tokio thread'lerine **isim verir** (`tokio-rt-worker`) — production'da `ps -T`/`top -H` teşhisini keyifli yapar; ve process toplam 3 thread tutar — 1 main (`block_on`'da bloklu) + 2 worker.

Cgroup koşuları (aynı sond, kafesteki shell):

```
$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max     # 0.5 vCPU
$ ./target/release/tokioburn
available_parallelism: Ok(1)
tokio workers: 1  (PID: 16611)
$ ps -T -p 16611
  16611   16611  tokioburn
  16611   16612  tokio-rt-worker          ← artık tek worker

$ echo "150000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max    # 1.5 vCPU
$ ./target/release/tokioburn
available_parallelism: Ok(1)
tokio workers: 1  (PID: 16841)
```

### Sonuçlar

| Ortam | `available_parallelism()` | tokio workers |
|---|---|---|
| çıplak | 2 | 2 |
| quota 0.5 vCPU | 1 | 1 |
| quota 1.5 vCPU | 1 | **1** — yine floor |

### Öğrettikleri

1. **tokio havuzunu `available_parallelism()`'den boyutlandırır** — Deney 3.7'nin tüm bulguları buraya aktarılır: quota-aware, affinity-aware, floor semantiği.
2. **"Quota-aware" = k8s *limit*'i demek, asla *request* değil.** `limits: cpu` → `cpu.max` olur, runtime okuyabilir; `requests: cpu` → `cpu.weight` olur — bir çekişme payıdır, ondan CPU sayısı türetilemez bile. Runtime request'i göremez:

| Pod ayarı (64 vCPU'lu node) | tokio workers |
|---|---|
| `limit: 500m` | 1 |
| `limit: 2` | 2 |
| `limit: 1500m` | 1 (floor) |
| **limit yok, sadece `request: 2`** | **64** |

3. **Yakalayıcı satır sonuncusu.** "Latency-kritik servise limit koyma" stratejisinin gizli maliyeti: okunacak quota yoksa runtime node'un CPU sayısına düşer ve 64 worker açar. Sakin node'da bu bedava burst kapasitesidir; kalabalık node'da `cpu.weight` o 64 thread'i ~2 CPU'luk paya sıkıştırır — run queue kalabalığı, §3.6'nın A-kaynağı stall'ları. Limitsiz koşarken çare: worker sayısını elle ver (`Builder::worker_threads(n)` ya da `TOKIO_WORKER_THREADS` env var), request'ine yakın boyutlandır.

[↑ Go back to TOC](#i̇çindekiler)

# Bölüm 5 — Kubernetes requests & limits *(yakında)*

Mekanizma, uçtan uca: Bölüm 3'ün burner'ları OpenShift'te pod olarak (statik musl binary, `FROM scratch` imaj), deney matrisi bu kez YAML ile. kubelet'in kurduğu cgroup ağacında gezinti (`kubepods.slice/...`), `requests`/`limits`'in Bölüm 2'deki dosyalara birebir eşlenmesi, elle ölçtüğümüz hücrenin (`echo "50000 100000" > cpu.max` → 241 M iter/s) Kubernetes'in `limits: cpu: 500m`'den kurduğu hücreyle aynı olduğunun teyidi ve her request/limit kombinasyonunun her iş yükü tipi için *faydalı / zararlı / etkisiz* olarak yargılanması.

[↑ Go back to TOC](#i̇çindekiler)

# Bölüm 6 — Performans lab'ı: Redis & Dragonfly boyutlandırma *(yakında)*

Hasat bölümü: gerçek engine'ler, gerçek yük, ölçülmüş boyutlandırma reçeteleri — VM'de elle kurulan cgroup'larla, OpenShift'te requests/limits ile. Zıt mimarili iki engine — Redis (tek thread'li event loop ≈ bizim 1-thread satırı) ve Dragonfly (thread-per-core ≈ bizim 8-thread satırı) — aynı yük altında, CPU kısıtları taranarak. Birbirini çapraz doğrulayan iki alet:

- **Tip A:** Bölüm 4'te yazdığımız tokio RESP client'ı — *uygulamanın gerçek write path'ini* modeller, yük deseni tamamen kontrolümüzde.
- **Tip B:** `memtier_benchmark` (Redis Ltd.'nin standart benchmark imajı) — dünyanın güvendiği endüstri referansı.

### Test topolojisi: ölçen asla aç kalmamalı

CPU'su biten bir yük üreteci, kendi acısını server'ın latency'si diye raporlar. Bu yüzden client, iki ortamda da server'dan **ayrı donanımda** yaşar:

```
VM senaryosu (elle cgroup):            OpenShift senaryosu (requests/limits):

  VM 1                 VM 2              node 1                node 2
┌─────────────┐     ┌──────────────┐   ┌─────────────┐     ┌──────────────┐
│ RESP client │ ──> │ redis /      │   │ client pod  │ ──> │ server pod   │
│ / memtier   │     │ dragonfly    │   │ (bol kaynak,│     │ (test edilen │
│ (limitsiz)  │     │ (cpu.max     │   │  limitsiz)  │     │  requests/   │
│             │     │  taranır)    │   │             │     │  limits)     │
└─────────────┘     └──────────────┘   └─────────────┘     └──────────────┘
```

Sabitler ve değişkenler sıkı ayrılır: network yolu sabit (aynı VM çifti / aynı node çifti, aynı AZ), client hep kısıtsız ve **koşudan koşuya değişen tek şey server'ın CPU kısıtı**. Her koşu aynı üçlüyü kaydeder: client tarafında p99, client tarafında throughput, server tarafında `cpu.stat` / `container_cpu_cfs_throttled_periods_total`. Sonuncusuyla ilkinin korelasyonu, bu lab'ın imza hareketidir.

Teslimat: ölçülmüş boyutlandırma reçeteleri — "şu yük deseni için Redis'e *şu* request/limit, Dragonfly'a *bu*; işte kanıtlayan sayılar."

[↑ Go back to TOC](#i̇çindekiler)
