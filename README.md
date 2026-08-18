<div align="right">

**English** | [Türkçe](README.tr.md)

</div>

# vCPU & cgroup Lab

A hands-on lab exploring CPU virtualization and resource control: Linux cgroup v2 experiments, Rust load generators, and Kubernetes CPU requests/limits — tracing how CPU time is allocated at each layer, from hardware threads to container quotas.

Every experiment here was run on a real machine (AWS EC2 `t3.large`, Ubuntu, cgroup v2) and the outputs shown are real measurements, not idealized numbers.

## Curriculum

| # | Part | Question it answers | Status |
|---|------|--------------------|--------|
| 1 | [CPU fundamentals](#part-1--cpu-fundamentals) | What is a core, a hyperthread, a vCPU — and who schedules whom? | ✅ |
| 2 | [cgroup v2 by hand](#part-2--cgroup-v2-by-hand) | How does the kernel slice CPU time, and how do I watch it happen? | ✅ |
| 3 | Rust load generator | How do thread count, concurrency and parallelism interact with vCPUs — measured, not guessed? | 🔜 |
| 4 | Kubernetes requests/limits | How do `requests`/`limits` translate to cgroup files, and why does `available_parallelism()` lie? | 🔜 |

---

# Part 1 — CPU Fundamentals

## 1.1 Four layers called "CPU"

Everyone says "CPU" while talking about four different things:

| Layer | Unit | Managed by |
|---|---|---|
| Hardware | physical core | — |
| Hardware | logical CPU (hyperthread) | the CPU itself |
| Virtualization | **vCPU** | hypervisor (KVM / Xen / Nitro) |
| Container | CPU quota | Linux cgroup (CFS scheduler) |

A useful mental model:

> **core** = the muscle · **vCPU** = your turn to use the muscle · **cgroup quota** = how long you may hold it while it's your turn.

## 1.2 Cores and SMT (Hyper-Threading)

- A **core** is a real execution engine: it runs one instruction stream at a time.
- **SMT** (*Simultaneous Multi-Threading*; Intel brand name: Hyper-Threading) gives one core **two architectural register sets** while sharing the execution units. When one thread stalls (e.g. a cache miss), the core runs the other.
- **Register set**: a handful of ultra-fast cells inside the CPU holding a thread's live state — which instruction it is at, the values in flight. **Execution units**: the circuits that do the actual work (ALU for arithmetic, load/store units for memory).
- Analogy: **one kitchen (execution units), two order boards (register sets)**. The cook works on one order while the other waits for ingredients. Two boards ≠ two kitchens: SMT yields roughly **+20–30 % throughput**, not 2×.
- Linux presents each hyperthread as a separate logical CPU.

## 1.3 What a vCPU actually is

**A vCPU is not a piece of hardware. It is a thread that the hypervisor schedules onto host CPUs.**

- To the hypervisor, every vCPU of your VM is just a task in its own scheduler — on AWS (KVM-based Nitro), your "CPU" is literally a thread inside another Linux kernel's run queue.
- On most AWS instance types, **1 vCPU = 1 hyperthread**. So `t3.large` = 2 vCPU = **1 physical core**.
- On-prem virtualization commonly **overcommits**: a host with 8 cores may hand out 40 vCPUs across VMs. It works because VMs are rarely all busy at once. A vCPU is a *right to run*, not a *guarantee*.

## 1.4 What the guest OS sees

- The guest kernel treats vCPUs as ordinary CPUs: `nproc` prints the vCPU count.
- Virtualization leaks through in one honest metric: **steal time** (`%st` in `top`) — "my vCPU was ready to run, but the hypervisor didn't give it physical CPU time." It is a hypervisor-level concept, unrelated to cgroups.
- Burstable instances (t3 etc.) are the easiest place to observe steal: exhaust the CPU credits (in Standard mode) and `%st` rises.

## 1.5 The Kubernetes "cpu" unit (preview)

- `cpu: 1` means **one vCPU's worth of time per scheduling period**, not a dedicated core.
- `requests` → a *weight* (your share when there is contention); `limits` → a *ceiling* (quota → throttling).
- Trap: inside a pod, `nproc` still reports the **node's** vCPU count — limits are invisible to it. This is why Rust's `available_parallelism()` misleads (Part 4).

## 1.6 The commands

| Command | What it tells you |
|---|---|
| `lscpu` | CPU **topology**: sockets, cores per socket, threads per core, model, caches |
| `lscpu -e` | One row per logical CPU with its `CORE` column — reveals which pairs share a core |
| `nproc` | Number of **logical** CPUs, nothing else |
| `top` | Live per-process `%CPU`; header row has `%st` (steal) and `id` (idle) |
| `cat /sys/fs/cgroup/cgroup.controllers` | Exists ⇒ cgroup **v2**; contents = resources the kernel can partition |
| `mount \| grep cgroup` | Which filesystem type is mounted where (`cgroup2` ⇒ v2) |

## 1.7 Reading a real machine (t3.large)

```
$ lscpu | head -20
CPU(s):                2
Thread(s) per core:    2        ← SMT is on
Core(s) per socket:    1        ← 1 socket × 1 core = 1 physical core
Socket(s):             1
Model name:            Intel(R) Xeon(R) Platinum 8259CL CPU @ 2.50GHz
Hypervisor vendor:     KVM      ← we are a VM, host runs KVM (Nitro)
Flags:                 ... ht ... hypervisor ...   ← 'hypervisor' bit: kernel knows it's virtualized
```

Deductions:

| Question | Answer | Evidence |
|---|---|---|
| Physical cores? | **1** | `Socket(s) × Core(s) per socket = 1 × 1` |
| SMT enabled? | **yes** | `Thread(s) per core: 2` |
| Logical CPUs (vCPUs)? | **2** | `CPU(s): 2`, confirmed by `nproc → 2` |
| Are we virtualized? | **yes, KVM** | `Hypervisor vendor` + `hypervisor` flag |

```
$ cat /sys/fs/cgroup/cgroup.controllers
cpuset cpu io memory hugetlb pids rdma misc
```

The file exists → this machine uses **cgroup v2**, and the `cpu` + `cpuset` controllers we need for Part 2 are available.

---

# Part 2 — cgroup v2 by hand

Kubernetes' `limits: cpu: 500m` is, under the hood, one small file write into a cgroup directory. In this part we lift the curtain and do that write ourselves, so that by the time we reach Kubernetes there is no magic left.

## 2.1 Talking to the kernel through files

`/sys/fs/cgroup` is **not a directory of files on disk**. It is a *pseudo-filesystem* (like `/proc`): hooks the kernel exposes in file form.

- **Reading** a file = querying the kernel live (`cat cpu.stat` computes counters at that instant).
- **Writing** a file = issuing a command (`echo "50000 100000" > cpu.max` applies the limit immediately).

This is Unix's *everything is a file* philosophy — no special API or syscalls needed; `cat` and `echo` are the entire toolchain.

```
$ mount | grep cgroup
cgroup2 on /sys/fs/cgroup type cgroup2 (rw,nosuid,nodev,noexec,relatime,...)
```

The `cgroup2` filesystem is mounted at `/sys/fs/cgroup`. Cgroups can only be created *inside this filesystem* — a `mkdir` anywhere else is just an empty directory on disk.

## 2.2 The layout: directories are cgroups

```
$ ls /sys/fs/cgroup/
cgroup.controllers  cgroup.procs  cgroup.subtree_control  cpu.stat ...   ← control files (this cgroup = root)
init.scope/  system.slice/  user.slice/                                  ← child cgroups (created by systemd)
```

- Every **directory** is a cgroup; nesting directories nests cgroups into a **tree**.
- Your machine already runs inside this tree: systemd puts services under `system.slice/`, your SSH session under `user.slice/`.
- `mkdir` inside the tree = *create a cgroup*. The kernel instantly populates the new directory with its control files. `rmdir` destroys it.

```
$ sudo mkdir /sys/fs/cgroup/lab
$ ls /sys/fs/cgroup/lab/ | head -5
cgroup.controllers
cgroup.events
cgroup.freeze
cgroup.kill
cgroup.max.depth        ← nobody created these; the kernel did, at mkdir time
```

## 2.3 The `subtree_control` gate

```
$ cat /sys/fs/cgroup/cgroup.controllers        # what exists on this kernel
cpuset cpu io memory hugetlb pids rdma misc
$ cat /sys/fs/cgroup/cgroup.subtree_control    # what the root grants to its children
cpuset cpu io memory pids
```

A cgroup's `cpu.*` files **only exist if its parent's `subtree_control` contains `cpu`**. The root has it enabled (courtesy of systemd); in your own parent cgroups you must enable it yourself:

```bash
echo "+cpu +cpuset" | sudo tee /sys/fs/cgroup/<parent>/cgroup.subtree_control
```

(`+` grants, `-` revokes.) Forgetting this is the classic "file not found: cpu.max" failure.

Related v2 rule (**no internal processes**): once a cgroup grants controllers to children, processes may only live in its **leaves**, never in the parent itself. This is why Kubernetes keeps pods at the bottom of its tree.

## 2.4 Reading the cgroup files

The four files you will read constantly. One-line summary:

> **`cpu.max` = the rule · `cgroup.procs` = the inmates · `/proc/<pid>/cgroup` = an inmate's ID card · `cpu.stat` = the ledger.**

### `cpu.max` — the rule

```
$ cat /sys/fs/cgroup/lab/cpu.max
max 100000
```

Format: `<quota> <period>`, both in microseconds. Time is chopped into repeating windows of `period` µs; within each window, the cgroup's processes may consume at most `quota` µs of CPU time **in total, across all CPUs**. On exhaustion the kernel freezes them until the next window — that is *throttling*.

| Value | Meaning |
|---|---|
| `max 100000` | unlimited (default; `max` is a keyword) — a pod without a CPU limit |
| `50000 100000` | 50 ms per 100 ms = **0.5 vCPU** — exactly what Kubernetes writes for `500m` |
| `200000 100000` | **2 vCPU** |
| `5000 10000` | also 0.5 vCPU, but with 10 ms windows → shorter freezes, lower latency impact |

The period is yours to choose (kernel accepts 1 ms – 1 s; 100 ms is the default and the Kubernetes default). **The ratio sets the average speed; the period sets the freeze granularity.**

```
|—— window 1 (100 ms) ——|—— window 2 (100 ms) ——|
[■■■■ run 50ms ][ frozen ][■■■■ run 50ms ][ frozen ]     ← cpu.max = 50000 100000
```

`top` shows such a process at 50 % — but the truth is it runs at *full speed half the time*.

### `cgroup.procs` — the inmates

```
$ cat /sys/fs/cgroup/lab/cgroup.procs
11254
```

One PID per line: who is in this cgroup right now. Writing a PID into it **moves** that process in; limits apply from that instant. No restart, no signal — the process never notices.

### `/proc/<pid>/cgroup` — the inmate's ID card

```
$ cat /proc/11254/cgroup
0::/lab
```

Same fact from the process's side: its path relative to the cgroup root. `0::/` = in the root; `0::/user.slice/...` = an ordinary login process.

### `cpu.stat` — the ledger

```
$ cat /sys/fs/cgroup/lab/cpu.stat
usage_usec 25900043      ← total CPU time consumed (µs) ≈ 25.9 s
user_usec 25877331       ←   ... in user code
system_usec 22712        ←   ... inside the kernel (syscalls)
nr_periods 665           ← how many quota windows have elapsed
nr_throttled 663         ← in how many of them the group was frozen
throttled_usec 40255281  ← total frozen time ≈ 40 s
```

- **Throttle ratio** = `nr_throttled / nr_periods` → here 663/665 ≈ **99.7 %**: this group hits the wall in virtually every window.
- Counters are **cumulative** since cgroup creation — for a rate, read twice and diff.
- Production health rule: a steadily climbing `nr_throttled` means the limit is too tight.

## 2.5 Experiment 1 — throttling with `cpu.max`

**Goal:** put a CPU-hungry process in a cgroup, cap it to half a vCPU, and watch the kernel enforce the cap — live, and in the ledger.

```bash
# 1. Create the cell
sudo mkdir /sys/fs/cgroup/lab

# 2. Start the load, note its PID
bash -c 'while :; do :; done' & echo "PID: $!"

# 3. Set the rule: 50 ms per 100 ms = half a vCPU
echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max

# 4. Move the process into the cell (the limit bites at this instant)
echo <PID> | sudo tee /sys/fs/cgroup/lab/cgroup.procs

# 5. Watch: %CPU ≈ 50 in top; confession in cpu.stat
top -p <PID>
cat /sys/fs/cgroup/lab/cpu.stat        # nr_throttled climbing?

# 6. Play live — no restart needed:
echo "20000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max   # → ~20 %
echo "max 100000"   | sudo tee /sys/fs/cgroup/lab/cpu.max   # → back to 100 %

# 7. Clean up
kill <PID>
sudo rmdir /sys/fs/cgroup/lab
```

Measured result (steps 5–6):

```
  PID USER  ...  %CPU  COMMAND
11254 ubuntu ... 100.0  bash        ← before joining the cgroup
11254 ubuntu ...  50.0  bash        ← after; then 20.0 after step 6

$ cat /sys/fs/cgroup/lab/cpu.stat
nr_periods 665
nr_throttled 663          ← frozen in 663 of 665 windows
throttled_usec 40255281   ← ~40 s spent frozen
```

Note the order of steps 3–4 doesn't matter: the limit belongs to the *cell*, and applies to whoever is inside the moment they enter.

## 2.6 Experiment 2 — `cpu.weight`, the share

**`cpu.max` is a ceiling ("never more than this"). `cpu.weight` is a share ("this is your cut *when there's a fight*; if it's quiet, take everything").**

- Range 1–10000, default **100**. Relative, not absolute: weight 300 beats weight 100 three-to-one.
- On an idle CPU, weight is irrelevant — even weight 1 gets 100 %. Weight never throttles and never increments `nr_throttled`.
- Kubernetes: `requests: cpu` is translated to `cpu.weight`. This experiment is "requests vs limits" at kernel level.

**Design problem:** we have 2 vCPUs. Two busy loops would land on separate vCPUs and never fight — and weight only referees fights. So we force the fight by pinning both cgroups to CPU 0 with **`cpuset.cpus`** (third controller unlocked: `cpuset` = *where* you may run, `cpu.max` = *how much*, `cpu.weight` = *who wins the queue*).

```bash
# 1. Two cells
sudo mkdir /sys/fs/cgroup/w100-lab /sys/fs/cgroup/w300-lab

# 2. Pin both to CPU 0 → guaranteed contention
echo 0 | sudo tee /sys/fs/cgroup/w100-lab/cpuset.cpus
echo 0 | sudo tee /sys/fs/cgroup/w300-lab/cpuset.cpus

# 3. Shares: w100-lab stays at default (100), w300-lab gets 300
echo 300 | sudo tee /sys/fs/cgroup/w300-lab/cpu.weight

# 4. Two identical loads
bash -c 'while :; do :; done' & echo "PID_100: $!"
bash -c 'while :; do :; done' & echo "PID_300: $!"

# 5. One into each cell
echo <PID_100> | sudo tee /sys/fs/cgroup/w100-lab/cgroup.procs
echo <PID_300> | sudo tee /sys/fs/cgroup/w300-lab/cgroup.procs

# 6. Watch both
top -p <PID_100> -p <PID_300>

# 7. Proof that weight is not a limit: kill the 300, watch the 100
kill <PID_300>

# 8. Clean up
kill <PID_100>
sudo rmdir /sys/fs/cgroup/w100-lab /sys/fs/cgroup/w300-lab
```

Measured result (step 6) — the 100:300 ratio, live:

```
%Cpu(s): 50.6 us, ... 48.6 id     ← machine total: half busy — CPU 1 sits idle (cpuset!)
  PID USER  ...  %CPU  COMMAND
11664 ubuntu ...  75.1  bash      ← weight 300
11663 ubuntu ...  24.9  bash      ← weight 100
```

After step 7 the survivor jumped to **100 %**: the fight was gone, so the share had nothing to divide. A `cpu.max` cap would have kept it at its ceiling even on an idle machine — that is the entire difference between requests and limits.

## 2.7 Experiment 3 — hierarchy: `tree-lab`, in three acts

cgroups form a **tree**, and a parent's limit applies to the *sum* of its children. That is exactly Kubernetes' layout: `kubepods.slice/` → pod → container. This experiment builds a two-child tree and stages three acts — including one where our own prediction was wrong, which taught the sharpest lesson of the lab.

```
/sys/fs/cgroup/tree-lab/        ← parent: cpu.max caps the TOTAL
├── w100/                       ← child, weight 100
└── w300/                       ← child, weight 300
```

**Question:** with 2 vCPUs idle, can two children escape the parent's 0.5-vCPU total by running on different CPUs? And does weight split the parent's pie?

```bash
# ── SETUP ──────────────────────────────────────────────
sudo mkdir /sys/fs/cgroup/tree-lab

# Grant cpu AND cpuset to the children (skip this ⇒ those files won't exist in children)
echo "+cpu +cpuset" | sudo tee /sys/fs/cgroup/tree-lab/cgroup.subtree_control

sudo mkdir /sys/fs/cgroup/tree-lab/w100 /sys/fs/cgroup/tree-lab/w300
ls /sys/fs/cgroup/tree-lab/w100/ | grep cpu      # verify the files appeared

# The pie: half a vCPU for the whole subtree
echo "50000 100000" | sudo tee /sys/fs/cgroup/tree-lab/cpu.max

# Shares: w100 keeps default (100), w300 gets 300
echo 300 | sudo tee /sys/fs/cgroup/tree-lab/w300/cpu.weight

# Two identical loads, one into each LEAF (writing into the parent is rejected)
bash -c 'while :; do :; done' & echo "PID_100: $!"
bash -c 'while :; do :; done' & echo "PID_300: $!"
echo <PID_100> | sudo tee /sys/fs/cgroup/tree-lab/w100/cgroup.procs
echo <PID_300> | sudo tee /sys/fs/cgroup/tree-lab/w300/cgroup.procs

# ── ACT A: quota without contention ────────────────────
top -p <PID_100> -p <PID_300>
cat /sys/fs/cgroup/tree-lab/cpu.stat     # throttling is accounted at the PARENT

# ── ACT B: pin both to CPU 0 — weight wakes up ─────────
echo 0 | sudo tee /sys/fs/cgroup/tree-lab/w100/cpuset.cpus
echo 0 | sudo tee /sys/fs/cgroup/tree-lab/w300/cpuset.cpus
top -p <PID_100> -p <PID_300>

# ── ACT C: grow the pie — weight alone on stage ────────
echo "100000 100000" | sudo tee /sys/fs/cgroup/tree-lab/cpu.max
top -p <PID_100> -p <PID_300>

# ── CLEANUP (children first — a non-empty dir won't rmdir) ──
kill <PID_100> <PID_300>
sudo rmdir /sys/fs/cgroup/tree-lab/w100 /sys/fs/cgroup/tree-lab/w300
sudo rmdir /sys/fs/cgroup/tree-lab
```

Measured results:

| Act | Setup | w100 | w300 | Total |
|---|---|---|---|---|
| A | quota ½ vCPU, no cpuset | **25 %** | **25 %** | ~50 % |
| B | + both pinned to CPU 0 | **12.7 %** | **37.3 %** | ~50 % |
| C | + quota raised to 1 vCPU | **25.0 %** | **74.7 %** | ~100 % |

**Act A — why 25/25 and not 25/75?** We predicted 25/75 and were wrong. The processes escaped to separate vCPUs — but they did *not* escape the quota: **quota is charged to the cgroup's total, not per CPU**. Running on two CPUs simply burns the 50 ms budget at 2× speed (gone in 25 ms), then *both* freeze. But since they never queued on the *same* CPU, there was no fight — and **weight only referees fights on a CPU's queue**. Without contention, the budget is consumed first-come-first-served: an even split.

**Act B** — same 50 ms pie, but now both sit in CPU 0's queue → weight referees: 100:300 ⇒ 12.5/37.5 predicted, 12.7/37.3 measured.

**Act C** — the pie (100 ms) now exceeds what one CPU can serve (CPU 0 can only give 100 ms per 100 ms), so the quota becomes invisible and weight alone divides the CPU: 25/75.

> **The pie, the knife, the table:** `cpu.max` sets the size of the pie, `cpu.weight` is the knife that cuts it, `cpuset` chooses the table — and the knife only works when everyone sits at the same table.

The Kubernetes translation of Act A is a production classic: give a multi-threaded app `limit: 1` on a many-core node, and its threads spread across CPUs, burn the entire budget in a fraction of the period, then freeze together — latency spikes while average CPU looks "only" 100 %. We will measure exactly this with Rust in Part 3.

## 2.8 Part 2 takeaways

| Concept | One-liner |
|---|---|
| cgroup | A named cell in a kernel-managed tree; directories are cells, files are the API |
| `cpu.max` | Ceiling: `quota period` in µs; enforced by freezing (throttling); counted on the cgroup total across all CPUs |
| `cpu.weight` | Share: divides contested CPU among siblings; powerless without contention; never throttles |
| `cpuset.cpus` | Placement: which logical CPUs the cell may use at all |
| `subtree_control` | Parent's grant of controllers to children (`+cpu +cpuset`); without it, child control files don't exist |
| No-internal-process rule | Once a parent delegates controllers, processes live only in leaf cgroups |
| `cpu.stat` | Cumulative ledger; `nr_throttled/nr_periods` is your throttle ratio |
| K8s mapping | `requests` → `cpu.weight`, `limits` → `cpu.max`, pod tree → cgroup tree |

---

# Part 3 — Rust load generator *(in progress)*

> **Test machine reminder** (details in [§1.7](#17-reading-a-real-machine-t3large)): AWS EC2 `t3.large` — **1 physical core × 2 SMT threads = 2 vCPUs**, Intel Xeon 8259CL @ 2.50 GHz, Ubuntu, cgroup v2. Every number in this part is relative to those 2 vCPUs.

A self-measuring load generator: N threads, each counting work done per second. This part has three goals, each an experiment: the thread-count sweep vs 2 vCPUs (§3.4 — where the **concurrency vs parallelism** distinction gets measured, not just defined), the same sweep inside a throttled cgroup (the thread × quota matrix), and `std::thread::available_parallelism()` vs reality.

## 3.1 Setting up the Rust toolchain on the VM

Install via **rustup** (the Rust project's official installer), not the distro package — `apt install cargo` ships a version that is typically a year or more behind, while rustup gives current stable and easy updates (`rustup update`).

```bash
# Install (as the regular user, no sudo needed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Load the environment into the current shell (new logins get it automatically)
. "$HOME/.cargo/env"

# Verify
cargo --version
```

Everything lands under `~/.cargo` and `~/.rustup` — no system files touched, removable with `rustup self uninstall`.

rustup may warn: `no default linker ('cc') was found in your PATH`. Rust compiles your code itself, but the final step — linking the binary — uses the system's C toolchain, which minimal cloud images don't ship. Install it, then smoke-test the toolchain end to end:

```bash
sudo NEEDRESTART_MODE=a apt update && sudo NEEDRESTART_MODE=a apt install -y build-essential
```

(`NEEDRESTART_MODE=a`: Ubuntu server ships the `needrestart` tool, which pops an interactive "which services should be restarted?" dialog after library upgrades; `a` = restart what's needed automatically, no questions.)

```bash
cargo new hello && cd hello && cargo run   # prints "Hello, world!" ⇒ toolchain complete
```

**Repo layout note:** the burner programs in this lab are single-file, std-only examples under [`burners/`](burners/), compiled directly with `rustc -O` — cargo would add nothing yet (no dependencies). Cargo returns the day we need crates (e.g. async experiments). The one risk of the cargo-less path: **`-O` is your responsibility now** — `rustc` does *not* optimize by default, and unoptimized benchmark numbers are garbage.

Don't confuse the two flags that appear side by side (and `-0`, digit zero, is not a flag at all):

```bash
rustc -O burners/02_threads.rs -o burn
#      ↑ capital O: Optimize        ↑ lowercase o: output — names the binary
```

## 3.2 First measurement — a self-measuring, single-thread burner

In Part 2 the referee was `top`, which shows CPU *occupancy*. From here on the program measures itself and reports *actual work done* — because under throttling, occupancy says "50 %" while only the work rate tells you what that costs. The code lives in this repo at [`burners/01_baseline.rs`](burners/01_baseline.rs):

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

Line by line:

| Code | What it does |
|---|---|
| `Instant::now()` | Starts the stopwatch. A *monotonic* clock — immune to wall-clock changes (NTP, DST). |
| `start.elapsed() < Duration::from_secs(secs)` | "Loop until 5 s have passed." The clock read is cheap — no syscall; it goes through the vDSO, in user space. |
| `count += 1` | The "work": increment a `u64` as fast as possible. |
| `count as f64 / secs / 1e6` | Normalizes to **M iter/s** — the common unit for every experiment that follows. |

Compile and run it both ways (binaries go to the git-ignored `burners/bin/`):

```bash
mkdir -p burners/bin
rustc    burners/01_baseline.rs -o burners/bin/01_baseline && ./burners/bin/01_baseline   # unoptimized
rustc -O burners/01_baseline.rs -o burners/bin/01_baseline && ./burners/bin/01_baseline   # optimized
```

Real output (t3.large):

```
131746254 iterations in 5 s  ->  26.3 M iter/s        ← unoptimized
169633279 iterations in 5 s  ->  33.9 M iter/s        ← optimized (-O)
```

**Measurement lesson #1.** Release is only ~29 % faster here — yet on pure compute loops it is routinely 10–100×. The explanation: every iteration calls `start.elapsed()`, so the loop's dominant cost is the clock read, which the optimizer cannot remove. As written, this program is closer to a "clock reads per second" benchmark than an "additions per second" one. You always measure *the most expensive thing in the loop* — know what that is. The next revision fixes this by checking the clock every N iterations instead of every one. The standing rule survives either way: **benchmark numbers only count from optimized builds (`rustc -O`).**

## 3.3 Version 2 — clean measurement, N threads

The revised burner is [`burners/02_threads.rs`](burners/02_threads.rs). Changes vs 01: the clock is checked once per 1 M iterations (the loop cost is now genuinely the counting), `std::hint::black_box` stops the optimizer from collapsing the count loop into a single addition, and the thread count comes as a CLI argument. The full code, annotated for a Rust newcomer:

```rust
use std::env;
use std::time::{Duration, Instant};

const BATCH: u64 = 1_000_000;            // how many counts between two clock reads

// The work of ONE thread: count for `secs` seconds, return the total.
fn burn(secs: u64) -> u64 {
    let start = Instant::now();
    let mut count: u64 = 0;
    while start.elapsed() < Duration::from_secs(secs) {   // clock read: once per BATCH
        for _ in 0..BATCH {
            // black_box = "compiler, you MUST really compute this".
            // Without it, -O would collapse the whole loop into `count += BATCH`
            // and we would measure nothing.
            count = std::hint::black_box(count + 1);
        }
    }
    count
}

fn main() {
    let secs = 5;

    // First CLI argument = thread count. `nth(1)` skips the program name;
    // if the argument is missing or not a number, fall back to 1.
    let threads: usize = env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);

    let start = Instant::now();

    // Start N identical threads. `move` hands each closure its own copy of `secs`.
    // Each `spawn` returns a JoinHandle — a ticket to collect that thread's result.
    let handles: Vec<_> = (0..threads).map(|_| std::thread::spawn(move || burn(secs))).collect();

    // `join()` blocks until the thread finishes and yields its return value.
    // Summing all tickets gives the total work done by all threads.
    let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();

    let wall = start.elapsed().as_secs_f64();   // wall-clock time actually elapsed

    let rate = total as f64 / wall / 1_000_000.0;
    println!("{threads} thread(s): {total} iters in {wall:.2} s  ->  {rate:.0} M iter/s total");
}
```

Execution model in one sentence: `main` spawns N workers that each count independently for 5 seconds (no shared data, no locks), then waits at the `join` line like a finish line, sums the per-thread totals, and divides by the *wall-clock* time — so the printed rate is machine throughput, not per-thread speed.

```bash
mkdir -p burners/bin
rustc -O burners/02_threads.rs -o burners/bin/02_threads
```

```bash
./burners/bin/02_threads 1     # 1 thread; the argument is the thread count (default 1)
```

Real output (t3.large), version 1 vs version 2, same machine, same "work":

```
$ ./burners/bin/01_baseline
172688794 iterations in 5 s   ->   34.5 M iter/s          ← v1: clock read every iteration
$ ./burners/bin/02_threads 1
1 thread(s): 2885000000 iters in 5.00 s  ->  577 M iter/s ← v2: clock read every 1 M iterations
```

**Measurement lesson #1, closed.** 17× difference — not because the machine got faster, but because v1 was effectively benchmarking clock reads, and v2 benchmarks the actual counting. (Sanity check: 577 M add/s on a 2.5 GHz core ≈ 4 cycles per iteration, plausible with the memory traffic `black_box` forces.) Fix the measurement before trusting the number. *(Thread sweep results: next.)*

## 3.4 Thread sweep — the parallelism wall, measured

One binary, one variable: the thread count. Plus one special run — 2 threads pinned to a single CPU — which separates concurrency from parallelism *with the thread count held constant*.

### Which number is the reference?

Two programs have appeared so far, and they play different roles:

- **`01_baseline.rs` is a teaching demo, not a reference.** Its job was to expose the *clock problem*: it read the clock on every iteration, and since a clock read costs ~17× more than the counting itself, its number (34.5 M iter/s) measured clock reads, not work (§3.2).
- **`./burn 1` (i.e. `02_threads.rs` with 1 thread) is the reference.** With the clock problem fixed (one clock read per million iterations), one thread on one vCPU produces **578 M iter/s** — "what one vCPU is worth." **Every comparison below is against this number**, never against 01.

### The tool: `taskset`

`taskset` sets a process's **CPU affinity**: the set of logical CPUs the kernel scheduler is allowed to place it on. `taskset -c 0 <cmd>` launches `<cmd>` allowed on CPU 0 only — the process's threads all inherit the restriction, so two busy threads have no choice but to take turns on one vCPU.

```bash
taskset -c 0 ./burn 2     # launch pinned to CPU 0 only
taskset -c 0,1 <cmd>      # launch allowed on CPUs 0 and 1 (ranges work too: -c 0-3)
taskset -cp <pid>         # show a running process's current affinity
taskset -cp 0 <pid>       # re-pin a running process to CPU 0, live
```

Why we need it here: without it, the scheduler immediately spreads 2 busy threads across our 2 vCPUs, and concurrency and parallelism rise together — indistinguishable. Pinning removes the parallelism while keeping the concurrency, which is exactly the variable we want to isolate. What the scheduler does in each case, on a timeline:

```
./burn 2  (both CPUs allowed)              taskset -c 0 ./burn 2  (CPU 0 only)
CPU 0: AAAAAAAAAAAAAAAA                    CPU 0: AAAA BBBB AAAA BBBB ...
CPU 1: BBBBBBBBBBBBBBBB                    CPU 1: (idle)
→ A and B truly simultaneous              → A and B take turns; at any instant
  (parallelism = 2)                          only one runs (parallelism = 1)
```

Both cases have concurrency = 2 (two threads in flight). Only the left one has parallelism = 2 — and only the left one is faster.

Relation to Part 2's `cpuset.cpus` — same kernel capability, two interfaces:

| | `taskset` | cgroup `cpuset.cpus` |
|---|---|---|
| Scope | one process (+ its threads/children) | every process in the cgroup |
| Privileges | none needed (own processes) | root (cgroup filesystem writes) |
| Enforcement | advisory — the process itself may call `sched_setaffinity()` and escape | mandatory — the cgroup wall cannot be escaped from inside |
| Typical use | quick experiments, benchmarks | containers, Kubernetes static CPU manager |

```bash
rustc -O burners/02_threads.rs -o burn
./burn 1
./burn 2
taskset -c 0 ./burn 2
./burn 4
./burn 8
```

Real output (t3.large — 2 vCPUs = 2 SMT threads of **one** physical core):

```
1 thread(s): 2889000000 iters in 5.00 s  ->  578 M iter/s total
2 thread(s): 4777000000 iters in 5.00 s  ->  955 M iter/s total
2 thread(s): 2886000000 iters in 5.00 s  ->  577 M iter/s total   ← taskset -c 0
4 thread(s): 4648000000 iters in 5.00 s  ->  929 M iter/s total
8 thread(s): 4249000000 iters in 5.01 s  ->  848 M iter/s total
```

| Run | Concurrency | Parallelism | M iter/s | vs `./burn 1` |
|---|---|---|---|---|
| `./burn 1` | 1 | 1 | 578 | **1.00× (reference)** |
| `./burn 2` | 2 | 2 | **955** | **1.65×** — not 2×! |
| `taskset -c 0 ./burn 2` | 2 | **1** | **577** | **1.00×** |
| `./burn 4` | 4 | 2 | 929 | 1.61× |
| `./burn 8` | 8 | 2 | 848 | 1.47× |

### Run by run

**`./burn 1` — the reference.** One thread, one vCPU busy, one idle. Purpose: establish the unit "what one vCPU is worth" (578 M iter/s). Every other row is judged against this number.

**`./burn 2` — full parallelism.** Two threads, two vCPUs, no restrictions — the machine's best case. Naive expectation: 2× = 1156. Measured: **955 = 1.65×**. The missing 35 % is SMT: these two vCPUs are the two order boards of *one* kitchen (§1.2); the threads share the core's execution units. The SMT bonus is workload-dependent (rule of thumb +20–30 %; this add-loop got +65 %), but it is never +100 %. **"2 vCPUs" ≠ "2 cores" — here is the measurement.**

**`taskset -c 0 ./burn 2` — concurrency without parallelism.** The control experiment: same two threads, but confined to CPU 0, so they *take turns* instead of running simultaneously. Concurrency is 2; parallelism is 1. Measured: **577 ≈ the 1-thread baseline exactly**. Doubling concurrency moved the work rate by nothing. **Parallelism gives speed; concurrency only gives structure** (useful for overlapping waits — but there is nothing to wait for in a pure compute loop).

**`./burn 4` and `./burn 8` — past the wall.** More threads than the hardware can run at once: parallelism stays pinned at 2 while concurrency grows. Expectation: flat at ~955. Measured: **929 and 848 — flat, then sagging** (~11 % below the peak at 8 threads).

Where does the loss come from? Follow the mechanics:

1. **8 runnable threads ÷ 2 logical CPUs = ~4 threads queued per CPU.** Everyone wants to run all the time; the hardware can seat two.
2. **The scheduler enforces fairness by rotation.** Linux's scheduler (CFS, and EEVDF in newer kernels) gives each thread a short time slice, then swaps: lift A off the CPU, seat B, a few milliseconds later lift B, seat C… Every thread advances, none quickly.
3. **Each swap — a *context switch* — has two costs.** The **direct** cost is small and visible: save A's registers, load B's, run the scheduler's bookkeeping — on the order of microseconds. The **indirect** cost is the sneaky one: while A was running, the CPU's L1/L2 caches filled with *A's* data. B arrives to a cold cache and stalls on memory until it re-warms — cycles that produce no work, and they don't appear in any "context switch time" statistic.
4. **The friction analogy.** The mass being pushed (total work) is unchanged; only the number of contact points (switches) grew — and every contact converts a little energy into heat. 955 → 848 is that heat: an **11 % friction loss**.

**Beyond the wall, threads don't just stop helping — they start costing.**

A subtlety that makes our −11 % a *best case*: these iterations are fully independent — no shared data, no locks, a tiny working set. Real applications share state; their threads evict each other's cache lines and queue on locks, so the oversubscription friction is typically far heavier than this clean loop shows.

### The lessons

1. **SMT inflates the CPU count**: capacity planning that reads `nproc` and assumes "N vCPUs = N cores of work" over-promises by design.
2. **Thread count is a statement about structure, not speed**; speed is capped by available parallel hardware.
3. **Oversubscription has a price** — and in Part 3's cgroup experiments this price will grow teeth: 8 threads against a capped quota burn the budget 8× faster, then freeze together.

# Part 4 — Kubernetes requests & limits *(coming soon)*

Walking the cgroup tree that kubelet builds (`kubepods.slice/...`), mapping `requests`/`limits` to the exact files from Part 2, observing CFS throttling on real pods on OpenShift, and why runtime thread-pool sizing needs to be cgroup-aware.
