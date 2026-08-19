<div align="right">

**English** | [Türkçe](README.tr.md)

</div>

# vCPU & cgroup Lab

A hands-on lab exploring CPU virtualization and resource control: Linux cgroup v2 experiments, Rust load generators, and Kubernetes CPU requests/limits — tracing how CPU time is allocated at each layer, from hardware threads to container quotas.

Every experiment here was run on a real machine (AWS EC2 `t3.large`, Ubuntu, cgroup v2) and the outputs shown are real measurements, not idealized numbers.

## Table of contents

- [Part 1 — CPU Fundamentals](#part-1--cpu-fundamentals)
  - [1.1 Four layers called "CPU"](#11-four-layers-called-cpu)
  - [1.2 Cores and SMT (Hyper-Threading)](#12-cores-and-smt-hyper-threading)
  - [1.3 What a vCPU actually is](#13-what-a-vcpu-actually-is)
  - [1.4 What the guest OS sees](#14-what-the-guest-os-sees)
  - [1.5 The Kubernetes "cpu" unit (preview)](#15-the-kubernetes-cpu-unit-preview)
  - [1.6 The commands](#16-the-commands)
  - [1.7 Reading a real machine (t3.large)](#17-reading-a-real-machine-t3large)
- [Part 2 — cgroup v2 by hand](#part-2--cgroup-v2-by-hand)
  - [2.1 Talking to the kernel through files](#21-talking-to-the-kernel-through-files)
  - [2.2 The layout: directories are cgroups](#22-the-layout-directories-are-cgroups)
  - [2.3 The `subtree_control` gate](#23-the-subtree_control-gate)
  - [2.4 Reading the cgroup files](#24-reading-the-cgroup-files)
  - [2.5 Experiment 1 — throttling with `cpu.max`](#25-experiment-1--throttling-with-cpumax)
  - [2.6 Experiment 2 — `cpu.weight`, the share](#26-experiment-2--cpuweight-the-share)
  - [2.7 Experiment 3 — hierarchy: `tree-lab`, in three acts](#27-experiment-3--hierarchy-tree-lab-in-three-acts)
  - [2.8 Part 2 takeaways](#28-part-2-takeaways)
- [Part 3 — Rust load generator](#part-3--rust-load-generator)
  - [3.1 Setting up the Rust toolchain on the VM](#31-setting-up-the-rust-toolchain-on-the-vm)
  - [Experiment 3.2 — first measurement: the clock problem](#experiment-32--first-measurement-the-clock-problem)
  - [Experiment 3.3 — clean measurement, N threads](#experiment-33--clean-measurement-n-threads)
  - [Experiment 3.4 — thread sweep: the parallelism wall](#experiment-34--thread-sweep-the-parallelism-wall)
  - [Experiment 3.5 — the thread × quota matrix](#experiment-35--the-thread--quota-matrix)
  - [Experiment 3.6 — stalls: the pain the matrix cannot see](#experiment-36--stalls-the-pain-the-matrix-cannot-see)
  - [Experiment 3.7 — who answers "how many CPUs?" honestly](#experiment-37--who-answers-how-many-cpus-honestly)
- [Part 4 — Async Rust: tokio and the vCPU](#part-4--async-rust-tokio-and-the-vcpu-in-progress)
  - [4.1 Cargo returns — project setup](#41-cargo-returns--project-setup)
  - [4.2 How tokio schedules — cooperative, `.await`, task queues](#42-how-tokio-schedules--cooperative-await-task-queues)
  - [Experiment 4.3 — how many workers does tokio start?](#experiment-43--how-many-workers-does-tokio-start)
  - [Experiment 4.4 — blocking the event loop, measured](#experiment-44--blocking-the-event-loop-measured)
  - [Experiment 4.5 — a million waiting tasks on two threads](#experiment-45--a-million-waiting-tasks-on-two-threads)
- [Part 5 — Kubernetes requests & limits](#part-5--kubernetes-requests--limits-coming-soon)
- [Part 6 — Performance lab: sizing Redis & Dragonfly](#part-6--performance-lab-sizing-redis--dragonfly-coming-soon)

## Curriculum

| # | Part | Question it answers | Status |
|---|------|--------------------|--------|
| 1 | [CPU fundamentals](#part-1--cpu-fundamentals) | What is a core, a hyperthread, a vCPU — and who schedules whom? | ✅ |
| 2 | [cgroup v2 by hand](#part-2--cgroup-v2-by-hand) | How does the kernel slice CPU time, and how do I watch it happen? | ✅ |
| 3 | [Rust load generator](#part-3--rust-load-generator) | How do thread count, concurrency and parallelism interact with vCPUs — measured, not guessed? | ✅ |
| 4 | [Async Rust (tokio)](#part-4--async-rust-tokio-and-the-vcpu-in-progress) | What do async tasks add on top of threads — and how does the worker pool interact with vCPUs and cgroup limits? | ⏳ |
| 5 | [Kubernetes requests/limits](#part-5--kubernetes-requests--limits-coming-soon) | How do `requests`/`limits` translate to cgroup files, and how do all the sync/async workloads behave under them? | 🔜 |
| 6 | [Performance lab (Redis & Dragonfly)](#part-6--performance-lab-sizing-redis--dragonfly-coming-soon) | What are the *right* CPU constraints for two opposite engine architectures — proven by measurement, on VM and OpenShift? | 🔜 |

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
- Trap: inside a pod, `nproc` still reports the **node's** vCPU count — limits are invisible to it. Whether your language runtime repeats that mistake or corrects it varies — measured in Experiment 3.7.

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

[↑ Go back to TOC](#table-of-contents)

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

How can a quota *larger* than the period fit — say `150000 100000`, 150 ms inside a 100 ms window? Because **the quota is charged against the cgroup's total across all CPUs**, not per CPU. On a 2-vCPU machine one window offers up to 2 × 100 = 200 ms of CPU time; two threads running simultaneously burn 150 ms of budget in 75 ms of wall-clock:

```
window:  |———————— 100 ms wall-clock ————————|
CPU 0:   [■■■■■■■ ran 75 ms ■■■■■■■][ frozen ]
CPU 1:   [■■■■■■■ ran 75 ms ■■■■■■■][ frozen ]
                                     total: 75+75 = 150 ms — budget spent
```

So `150000 100000` = 150/100 = **1.5 vCPUs' worth of time**. The rule of thumb: **divide the left number by the right one and you get the vCPU count.**

#### The ceiling: what is the largest useful left value?

Naming, once and for all — an `a b` entry means:

```
a = quota   → the BUDGET: CPU-time (µs) the cgroup may spend per window
b = period  → the WINDOW: wall-clock length (µs) of one accounting round
```

The budget is spent on **logical CPUs** — the things the kernel schedules onto. Their count comes from the hardware:

```
logical CPUs = physical cores × SMT factor
```

(SMT recap from §1.2: one physical core holding 2+ full register sets, so the OS sees it as 2+ CPUs; the execution machinery is shared. x86 SMT factor is 2; IBM POWER goes 4–8; Apple M / Graviton have no SMT, factor 1.)

Each logical CPU can contribute at most `b` µs of CPU-time per window — it cannot run more than wall-clock. Hence the ceiling:

```
max spendable quota per window:   a_max = b × logical CPUs
```

On this lab's t3.large, drawn out:

```
                    ┌─ physical core 0 ─┐
                    │  HT-A       HT-B  │        1 core × SMT 2 = 2 logical CPUs
                    └───┬───────────┬───┘
                        │           │
window (b = 100 ms):    │           │
CPU 0 = HT-A:  [■■■ up to 100 ms ■■■]  ┐
CPU 1 = HT-B:  [■■■ up to 100 ms ■■■]  ┴→  a_max = 2 × 100 ms = 200 ms
```

Worked examples, `b = 100000` (100 ms) everywhere:

| Machine | cores | SMT | logical CPUs | ceiling `a_max` | i.e. `cpu.max` beyond which more is meaningless |
|---|---|---|---|---|---|
| t3.large (this lab) | 1 | 2 | 2 | 200 ms | `200000 100000` |
| 4-core SMT Xeon | 4 | 2 | 8 | 800 ms | `800000 100000` |
| Graviton, 8 cores | 8 | 1 | 8 | 800 ms | `800000 100000` |
| POWER9, 4 cores SMT8 | 4 | 8 | 32 | 3200 ms | `3200000 100000` |

Three footnotes to the formula:

1. Writing a quota **above** the ceiling is legal — the kernel accepts `800000 100000` on the t3 — but the group physically cannot spend more than 200 ms, so anything past the ceiling behaves like `max`.
2. The multiplier is the logical CPUs **this cgroup can reach**: a `cpuset.cpus 0` pin shrinks the ceiling to `b × 1` no matter what the machine has.
3. The ceiling is a **time** ceiling, not a **work** ceiling: 200 ms spent on an SMT pair produces ~1.65 CPUs' worth of work, not 2 (§3.4/§3.5) — the ms are equal, the silicon behind them is not.

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

[↑ Go back to TOC](#table-of-contents)

---

# Part 3 — Rust load generator

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
rustc -O burners/02_threads.rs -o burners/bin/02_threads
#      ↑ capital O: Optimize        ↑ lowercase o: output — names the binary
```

## Experiment 3.2 — first measurement: the clock problem

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

## Experiment 3.3 — clean measurement, N threads

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

## Experiment 3.4 — thread sweep: the parallelism wall

One binary, one variable: the thread count. Plus one special run — 2 threads pinned to a single CPU — which separates concurrency from parallelism *with the thread count held constant*.

### Which number is the reference?

Two programs have appeared so far, and they play different roles:

- **`01_baseline.rs` is a teaching demo, not a reference.** Its job was to expose the *clock problem*: it read the clock on every iteration, and since a clock read costs ~17× more than the counting itself, its number (34.5 M iter/s) measured clock reads, not work (§3.2).
- **`02_threads 1` (i.e. `02_threads.rs` with 1 thread) is the reference.** With the clock problem fixed (one clock read per million iterations), one thread on one vCPU produces **578 M iter/s** — "what one vCPU is worth." **Every comparison below is against this number**, never against 01.

### The tool: `taskset`

`taskset` sets a process's **CPU affinity**: the set of logical CPUs the kernel scheduler is allowed to place it on. `taskset -c 0 <cmd>` launches `<cmd>` allowed on CPU 0 only — the process's threads all inherit the restriction, so two busy threads have no choice but to take turns on one vCPU.

```bash
taskset -c 0 burners/bin/02_threads 2     # launch pinned to CPU 0 only
taskset -c 0,1 <cmd>      # launch allowed on CPUs 0 and 1 (ranges work too: -c 0-3)
taskset -cp <pid>         # show a running process's current affinity
taskset -cp 0 <pid>       # re-pin a running process to CPU 0, live
```

Why we need it here: without it, the scheduler immediately spreads 2 busy threads across our 2 vCPUs, and concurrency and parallelism rise together — indistinguishable. Pinning removes the parallelism while keeping the concurrency, which is exactly the variable we want to isolate. What the scheduler does in each case, on a timeline:

```
02_threads 2  (both CPUs allowed)          taskset -c 0 02_threads 2  (CPU 0 only)
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
rustc -O burners/02_threads.rs -o burners/bin/02_threads
burners/bin/02_threads 1
burners/bin/02_threads 2
taskset -c 0 burners/bin/02_threads 2
burners/bin/02_threads 4
burners/bin/02_threads 8
```

Real output (t3.large — 2 vCPUs = 2 SMT threads of **one** physical core):

```
1 thread(s): 2889000000 iters in 5.00 s  ->  578 M iter/s total
2 thread(s): 4777000000 iters in 5.00 s  ->  955 M iter/s total
2 thread(s): 2886000000 iters in 5.00 s  ->  577 M iter/s total   ← taskset -c 0
4 thread(s): 4648000000 iters in 5.00 s  ->  929 M iter/s total
8 thread(s): 4249000000 iters in 5.01 s  ->  848 M iter/s total
```

| Run | Concurrency | Parallelism | M iter/s | vs `02_threads 1` |
|---|---|---|---|---|
| `02_threads 1` | 1 | 1 | 578 | **1.00× (reference)** |
| `02_threads 2` | 2 | 2 | **955** | **1.65×** — not 2×! |
| `taskset -c 0 02_threads 2` | 2 | **1** | **577** | **1.00×** |
| `02_threads 4` | 4 | 2 | 929 | 1.61× |
| `02_threads 8` | 8 | 2 | 848 | 1.47× |

### Run by run

**`02_threads 1` — the reference.** One thread, one vCPU busy, one idle. Purpose: establish the unit "what one vCPU is worth" (578 M iter/s). Every other row is judged against this number.

**`02_threads 2` — full parallelism.** Two threads, two vCPUs, no restrictions — the machine's best case. Naive expectation: 2× = 1156. Measured: **955 = 1.65×**. The missing 35 % is SMT: these two vCPUs are the two order boards of *one* kitchen (§1.2); the threads share the core's execution units. The SMT bonus is workload-dependent (rule of thumb +20–30 %; this add-loop got +65 %), but it is never +100 %. **"2 vCPUs" ≠ "2 cores" — here is the measurement.**

**`taskset -c 0 02_threads 2` — concurrency without parallelism.** The control experiment: same two threads, but confined to CPU 0, so they *take turns* instead of running simultaneously. Concurrency is 2; parallelism is 1. Measured: **577 ≈ the 1-thread baseline exactly**. Doubling concurrency moved the work rate by nothing. **Parallelism gives speed; concurrency only gives structure** (useful for overlapping waits — but there is nothing to wait for in a pure compute loop).

**`02_threads 4` and `02_threads 8` — past the wall.** More threads than the hardware can run at once: parallelism stays pinned at 2 while concurrency grows. Expectation: flat at ~955. Measured: **929 and 848 — flat, then sagging** (~11 % below the peak at 8 threads).

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

## Experiment 3.5 — the thread × quota matrix

Part 2 throttled a toy loop and watched `top`; now we throttle the *measuring instrument* and read real numbers. Two variables, one grid: thread count (1 / 2 / 8) × `cpu.max` (unlimited / 1 vCPU / 0.5 vCPU) — nine cells.

### Method: a caged shell

A benchmark run lasts 5 seconds — too short to catch its PID and move it into a cgroup mid-flight. The fix uses an inheritance rule: **a process is born into its parent's cgroup.** So we place the *shell* into the cage once; every command it launches afterwards starts inside from its first instruction.

The whole experiment runs in **one terminal**: once the shell is caged, the quota changes (`sudo tee`) and `cpu.stat` reads also run inside the cage — they are instantaneous and don't disturb the measurements.

```bash
# Create the cell, then cage this very shell ($$ is the shell's own PID)
sudo mkdir /sys/fs/cgroup/lab
echo $$ | sudo tee /sys/fs/cgroup/lab/cgroup.procs
cat /proc/$$/cgroup                    # must say 0::/lab

# For each column: set the quota, then run the three thread counts,
# reading the kernel's witness after EVERY run.
echo "max 100000"    | sudo tee /sys/fs/cgroup/lab/cpu.max   # column 1: unlimited
# (column 2: "100000 100000" = 1 vCPU · column 3: "50000 100000" = 0.5 vCPU)

burners/bin/02_threads 1
grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
burners/bin/02_threads 2
grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat
burners/bin/02_threads 8
grep -E 'nr_periods|nr_throttled' /sys/fs/cgroup/lab/cpu.stat

# Cleanup — gotcha included: rmdir will fail with "Device or resource busy"
# while your shell is still inside. Evict yourself first (move back to the root
# cgroup), then remove the cell:
echo $$ | sudo tee /sys/fs/cgroup/cgroup.procs
cat /proc/$$/cgroup                    # 0::/ — free again
sudo rmdir /sys/fs/cgroup/lab
```

Why read `cpu.stat` after every run, not once per column? The counters are **cumulative**; only per-run differences attribute the blame. `nr_periods` counts elapsed quota windows, `nr_throttled` counts the windows in which the group was frozen for exhausting its budget — the same counter Kubernetes exports to Prometheus as `container_cpu_cfs_throttled_periods_total`, the number-one metric behind every "my pod is slow" investigation.

### Results

The full run (t3.large), all nine cells with the witness read after every run:

```
$ echo "max 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max        # ── column 1: unlimited
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

$ echo "100000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max     # ── column 2: 1 vCPU
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

$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max      # ── column 3: 0.5 vCPU
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

| | unlimited | 1 vCPU | 0.5 vCPU |
|---|---|---|---|
| **1 thread** | 581 | 574 | 288 |
| **2 threads** | 858 | 489 | 241 |
| **8 threads** | 854 | 490 | 250 |

**Computing the kernel-witness deltas.** `cpu.stat` counters are cumulative — they never reset while the cgroup exists. So a single reading says nothing about *this* run; what belongs to a run is the **difference between the reading after it and the reading before it** (i.e. the previous run's reading). Worked through the raw transcript above:

| Run | before (periods / throttled) | after | Δ`nr_periods` | Δ`nr_throttled` |
|---|---|---|---|---|
| 1 vCPU, 1 thread | 324 / 249 | 379 / 249 | 55 | **0** |
| 1 vCPU, 2 threads | 379 / 249 | 433 / 296 | 54 | 47 |
| 1 vCPU, 8 threads | 433 / 296 | 488 / 344 | 55 | 48 |
| 0.5 vCPU, 1 thread | 488 / 344 | 548 / 394 | 60 | 50 |
| 0.5 vCPU, 2 threads | 548 / 394 | 603 / 444 | 55 | 50 |
| 0.5 vCPU, 8 threads | 603 / 444 | 660 / 495 | 57 | 51 |

(The unlimited column stayed frozen at 324 / 249 — those are leftovers from earlier experiments in the same cgroup; with no limit set, accounting doesn't even run. And the ~55 periods per run is no accident: a 5 s run ÷ 100 ms period ≈ 50 windows, plus a few windows of shell activity between commands.)

Kernel witness (per-run Δ`nr_throttled` / Δ`nr_periods`):

| | unlimited | 1 vCPU | 0.5 vCPU |
|---|---|---|---|
| **1 thread** | 0 / 0 | **0** / 55 | 50 / 60 |
| **2 threads** | 0 / 0 | 47 / 54 | 50 / 55 |
| **8 threads** | 0 / 0 | 48 / 55 | 51 / 57 |

Reading a cell: "47 / 54" = of the 54 quota windows that elapsed during this run, the group was frozen in 47 — throttled in 87 % of them.

*(Side note: in the unlimited column `nr_periods` did not advance at all — bandwidth accounting only runs while a limit is set. And the unlimited 2-thread cell measured 858 vs 955 in §3.4: t3 CPU credits were beginning to erode; watch `%st`.)*

### What the grid teaches

**Row 1 — quota is honest with a single thread.** 581 → 574 → 288: no measurable cost at 1 vCPU, exactly half at half a vCPU. And the subtle gem: at 1 vCPU, Δ`nr_throttled` = **0** — a single thread physically cannot occupy more than one CPU, so it never touches the budget ceiling. Kubernetes translation: **a limit larger than what the app can use is neutral** — it neither helps nor hurts.

**Column 2 — the bombshell: 2 threads under a 1-vCPU quota produce *less* than 1 thread (489 < 574).** Same budget, more workers, less work. Mechanics: two threads burn the 100 ms budget on both SMT siblings at 2× speed — gone in ~50 ms, frozen for the rest — but an SMT pair only produces ~1.65× while running. **A quota-millisecond spent on an SMT pair yields less work than one spent on a full CPU**, and the freeze/wake cycle adds its own tax. The kernel confirms: throttled in ~87 % of windows. This is the production riddle "we set `limit: 1` — why is the app slower than single-threaded?" answered with a grid cell.

**Column 3 — the painful cell.** One thread takes the clean half (288); 2 and 8 threads pay a further ~15 % tax (241–250) while pinning the throttle ratio to ~90 %. Throughput, however, is the *mild* symptom — the real damage of many-threads-under-tight-quota is the freeze pattern, which §3.6 measures directly.

**The sizing rule this grid proves:** under a CPU limit, match the thread count to the limit (⌈limit⌉ threads), not to what `nproc` claims. Extra threads under a tight quota are pure loss: same or less throughput, maximal throttling.

## Experiment 3.6 — stalls: the pain the matrix cannot see

Experiment 3.5 ended with "throughput halved" — unpleasant but tolerable. What the matrix could *not* show: under a tight quota, work does not flow slower-and-evenly. It flows in bursts separated by dead freezes. For a server, those freezes are requests sitting motionless in a queue.

**Why neither `top` nor throughput reveals this: both are averages.** `top` samples CPU usage over its refresh interval; our benchmark divides total work by 5 seconds. A 100 ms freeze disappears inside either average — "50 % CPU" can mean *everything runs at half speed* or *full speed half the time, dead the other half*. For throughput those are identical; for latency they are different worlds. The only observer that can tell them apart sits **inside the process**, timestamping its own progress. That is why the burner must measure its own stalls.

### The tool: `03_stalls.rs`

[`burners/03_stalls.rs`](burners/03_stalls.rs) is `02_threads` plus one idea: each thread remembers the **longest gap between two consecutive batch completions**. The heart of it, annotated:

```rust
fn burn(secs: u64) -> (u64, Duration) {          // now returns TWO things:
    let start = Instant::now();                  //   (total count, worst gap)
    let mut count: u64 = 0;
    let mut max_stall = Duration::ZERO;          // worst gap seen so far: starts at 0
    let mut last = Instant::now();               // timestamp of the PREVIOUS batch end

    while start.elapsed() < Duration::from_secs(secs) {
        for _ in 0..BATCH {                      // the batch: 1 M counted increments
            count = std::hint::black_box(count + 1);
        }
        let now = Instant::now();                // this batch just finished
        let gap = now - last;                    // time since the previous finish —
        if gap > max_stall {                     //   work time AND any freeze in between
            max_stall = gap;                     // keep only the record holder
        }
        last = now;                              // this finish becomes the new reference
    }
    (count, max_stall)
}
```

How the measurement works, step by step: the thread lives batch to batch, like a runner clocking laps. `last` always holds the finish time of the previous lap; each new finish computes `gap` = the lap's true duration — CPU work *plus* anything that interrupted it (a quota freeze, waiting in the run queue). On an unlimited, uncontended CPU every lap is ~2 ms, so `max_stall` stays ~2 ms. If the kernel froze the group for 50 ms mid-lap, that lap's `gap` jumps to ~52 ms and `max_stall` records it. In `main`, each thread returns its own maximum and the worst across threads is printed — the longest time *any* thread stood still.

One honest limitation: we keep only the *maximum*, so the output says how bad the worst moment was, not how often bad moments happened. (A real latency benchmark would keep a histogram and report p50/p99 — that refinement belongs to the tokio part.)

### The runs

Cage setup identical to §3.5 (shell in the cgroup, one terminal). Six runs: two thread counts (1 / 8) × three regimes — unlimited, 0.5 vCPU with the standard 100 ms period, and 0.5 vCPU with a 10 ms period (same ratio, 10× shorter windows).

```bash
rustc -O burners/03_stalls.rs -o burners/bin/03_stalls

burners/bin/03_stalls 1                                      # ── unlimited
burners/bin/03_stalls 8
echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max    # ── 0.5 vCPU, 100 ms period
burners/bin/03_stalls 1
burners/bin/03_stalls 8
echo "5000 10000" | sudo tee /sys/fs/cgroup/lab/cpu.max      # ── 0.5 vCPU, 10 ms period
burners/bin/03_stalls 1
burners/bin/03_stalls 8
```

Full run (t3.large):

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

### Results

| Run | M iter/s | Worst stall |
|---|---|---|
| unlimited, 1 thread | 578 | 2.1 ms |
| unlimited, 8 threads | 909 | **30.8 ms** |
| 0.5 vCPU · 100 ms period, 1 thread | 289 | 53.3 ms |
| 0.5 vCPU · 100 ms period, 8 threads | 237 | 115.1 ms |
| 0.5 vCPU · 10 ms period, 1 thread | 285 | **7.7 ms** |
| 0.5 vCPU · 10 ms period, 8 threads | 272 | **135.9 ms** |

### Anatomy: two sources of waiting

The six numbers look erratic until you see that "stall" is not one thing. A thread that isn't running is waiting on one of **two independent mechanisms**:

- **Source A — run-queue waiting.** More runnable threads than CPUs: the scheduler rotates them, and your thread waits *while others run*. Think of a barbershop: two chairs (vCPUs), eight customers (threads) — most of your visit is spent in the waiting row, not the chair.
- **Source B — the quota freeze.** The cgroup's budget for this window is spent; the kernel stops *everyone* until the next window opens. The barbershop's shutter comes down — no one is served, no matter how short the row is.

The two are independent: A needs contention (many threads), B needs a limit (quota). Each run in our table switches one, the other, or both:

| Run | Stall | Diagnosis |
|---|---|---|
| unlimited, 1 thread | 2.1 ms | neither — just a batch's own duration |
| unlimited, 8 threads | 30.8 ms | pure **A**: 4 threads per chair, ~3–4 turns of waiting |
| 0.5 vCPU · 100 ms, 1 thread | 53.3 ms | pure **B**: 50 ms shutter (period − quota) + ~3 ms batch |
| 0.5 vCPU · 100 ms, 8 threads | 115.1 ms | **A + B**: the shutter lifts — but it's still not your turn; next window, frozen again |
| 0.5 vCPU · 10 ms, 1 thread | 7.7 ms | pure B, miniaturized: 5 ms shutter + batch |
| 0.5 vCPU · 10 ms, 8 threads | 135.9 ms | **A × B, worst case**: each window opens for a 5 ms crumb, eight hungry threads fight for it — an unlucky thread can go whole *series* of windows without a turn |

The diagnostic rule that falls out: **in the 1-thread rows only B exists — the stall is predictable by formula and shrinks with the period. In the 8-thread rows A joins in and compounds with B — the stall now belongs to the length of the queue, and no period setting can shorten a queue.** That is why the same knob (10 ms period) healed one row (53 → 7.7) and did nothing for the other (115 → 136).

### What it teaches

1. **The ratio sets the speed; the period does not.** 289 vs 285, 237 vs 272 — the period changed 10×, throughput didn't move. Average speed is quota ÷ period, full stop.
2. **For a single thread, the period is the pain dial.** Worst stall ≈ (period − quota) + one batch: 53.3 ms ≈ 50 ms freeze + ~3 ms of work; shrink the period to 10 ms and the stall collapses to 7.7 ms — same throughput, 7× gentler tail latency. This is precisely kubelet's `cpuCFSQuotaPeriod` knob.
3. **Oversubscription is a latency machine even with no limit at all.** 8 threads, unlimited: 30.8 ms worst stall — nobody was frozen; that is pure turn-waiting among 8 threads on 2 vCPUs (§3.4's friction, seen from the latency side).
4. **And under a tight quota, it defeats the period cure.** We predicted the short period would rescue the 8-thread case too — it did not (115 → 136 ms). With 10 ms windows, the budget is a 5 ms crumb shared by 8 hungry threads; an unlucky thread waits through *both* queues — the freezes *and* its turn — across many consecutive windows. When queueing dominates, tuning the period is not the cure; **removing threads is**. (Wrong prediction #2 for this lab; both times the correction taught more than the guess.)

Kubernetes coda: this is the anatomy of "p99 exploded but CPU shows only 50 %" — the pod isn't slow, it's *stuttering*. Diagnosis: `container_cpu_cfs_throttled_periods_total` climbing. Cure, in order: match the thread count to the limit, then consider the period.

## Experiment 3.7 — who answers "how many CPUs?" honestly

Every runtime sizes its thread pool by asking the system "how many CPUs do I have?" — and Part 1 planted the warning that inside a container the answer can be a trap. Time to measure who lies and who doesn't. Three information layers exist, and each responder may read a different subset:

1. **Topology** — how many logical CPUs the machine has (`/proc`, sysfs).
2. **Affinity** — which CPUs this process is *allowed on* (`sched_getaffinity`; set by `taskset`/`cpuset`).
3. **cgroup quota** — how much CPU *time* the process may spend (`cpu.max`; set by Kubernetes limits).

The tool is six lines — [`burners/04_nproc.rs`](burners/04_nproc.rs), printing Rust std's official answer:

```rust
use std::thread;

fn main() {
    match thread::available_parallelism() {
        Ok(n) => println!("available_parallelism: {n}"),
        Err(e) => println!("error: {e}"),
    }
}
```

Four scenarios, `nproc` and the Rust answer side by side (cage setup as in §3.5):

```
$ nproc                                       # ── bare: no cgroup, no pinning
2
$ burners/bin/04_nproc
available_parallelism: 2

$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max    # ── caged, quota 0.5 vCPU
$ nproc
2
$ burners/bin/04_nproc
available_parallelism: 1

$ echo "150000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max   # ── caged, quota 1.5 vCPU
$ nproc
2
$ burners/bin/04_nproc
available_parallelism: 1

$ echo "max 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max      # ── no quota, pinned to CPU 0
$ taskset -c 0 nproc
1
$ taskset -c 0 burners/bin/04_nproc
available_parallelism: 1
```

### Results

| Scenario | `nproc` | `available_parallelism()` |
|---|---|---|
| bare | 2 | 2 |
| quota 0.5 vCPU | **2** | **1** |
| quota 1.5 vCPU | **2** | **1** |
| pinned to CPU 0 (`taskset`) | 1 | 1 |

### What it teaches

1. **`nproc` reads affinity, never quota.** Under a 0.5-vCPU limit it still says 2 — this is the layer that tells a pod on a 64-vCPU node "you have 64 CPUs". Every script and program that sizes by `nproc` inherits the blindness.
2. **Modern Rust std reads the quota too — the "lie" narrative is outdated for Rust.** Since ~1.61, `available_parallelism()` checks `cpu.max` along with affinity: under 0.5 vCPU it answers 1. Our prediction said "it misleads" — wrong prediction #3 for this lab, and the happiest one: the ecosystem had already learned this lesson.
3. **The 1.5-vCPU surprise: Rust rounds *down*.** quota/period = 1.5 → answer 1 (with a floor of 1). Conservative by design: two workers on 1.5 CPUs of budget would both throttle; one worker runs clean and leaves 0.5 unused. Latency is favored over utilization.
4. **The trap is dead only in some languages.** `nproc`-based scripts, plain C, Go's `GOMAXPROCS` (without automaxprocs), older JVMs still see the node's count — the "64 workers under `limit: 2`" incident remains real in mixed-language fleets. What tokio does, Part 4 will *measure*, not assume.

[↑ Go back to TOC](#table-of-contents)

# Part 4 — Async Rust: tokio and the vCPU *(in progress)*

The async chapter, with a real deliverable: the Part 3 burners measured the *sync* world; here we build their async counterpart — a **tokio-based RESP load generator** (a client that speaks the Redis protocol: pipelined SET/GET at a fixed rate, reporting p50/p99 latency). Network IO is where async actually earns its keep, so the tool and the lesson coincide. Along the way: tokio's runtime model (a few OS worker threads carrying many lightweight tasks), how many workers it starts inside a limited cgroup — measured, not assumed — and CPU-bound vs IO-bound tasks under the same quotas. Cargo returns here (tokio is an external crate).

## 4.1 Cargo returns — project setup

Parts 1–3 needed nothing beyond std, so plain `rustc -O` was enough. tokio is an external crate — this is what cargo exists for: it downloads the dependency (and its dependencies) from crates.io, compiles and caches them.

```bash
cargo new tokioburn && cd tokioburn
```

Add to `Cargo.toml` under `[dependencies]`:

```toml
tokio = { version = "1", features = ["full"] }
```

(`features = ["full"]` enables all tokio components — runtime, net, time. Production builds pick selectively; for a lab, full is practical.) First `cargo build --release` takes a minute or two while tokio compiles; subsequent builds are seconds. The project lives at [`tokioburn/`](tokioburn/) in this repo.

**Layout: one project, one binary per experiment.** Instead of overwriting `src/main.rs` for every experiment, the project uses cargo's multi-binary layout: each file under `src/bin/` compiles to its own binary, all sharing one `Cargo.toml` and one dependency build (tokio compiles once). `cargo build --release` builds them all; each runs as `./target/release/<file-name>`.

```
tokioburn/src/bin/
├── 01_workers.rs      ← experiment 4.3
├── 02_heartbeat.rs    ← experiment 4.4
└── ...                ← one probe per experiment, numbered like burners/
```

## 4.2 How tokio schedules — cooperative, `.await`, task queues

Theory first — five building blocks that everything in async Rust hangs on.

### Vocabulary and async concepts in Rust

Three words carry this whole part; fix them before anything else.

| Term | What it is | Who creates & manages it | Cost | How it's interrupted |
|---|---|---|---|---|
| **Thread** | An OS execution unit; the kernel schedules it onto logical CPUs | the kernel | ~MB of stack, syscalls to create | **preemptively** — the kernel forces it off the CPU |
| **Worker** | Not a new concept: an ordinary **thread that tokio creates** and dedicates to one job — loop forever, pull tasks from the task queues, run them. Visible in `ps -T` as `tokio-rt-worker` (Experiment 4.3) | tokio (at runtime startup; count from `available_parallelism()`) | same as any thread | same as any thread |
| **Task** | A unit of async work (`async { ... }` handed to `tokio::spawn`) — a *record of work to do*, not an execution unit; it runs **inside** whichever worker picks it up | tokio | ~hundreds of bytes — millions are fine | **cooperatively only** — nobody can force it off; it must reach an `.await` |

The two layers, stacked:

```
tasks           → managed by tokio  → run INSIDE worker threads
worker threads  → managed by kernel → run ON logical CPUs
```

One sentence to keep: **a task is a work record, a worker is the thread that executes such records, and the kernel only ever sees the workers.**

#### The two spawns: `tokio::spawn` vs `tokio::task::spawn_blocking`

Both hand work to the runtime and both return a `JoinHandle` you `.await` — but they send the work to different places, for different kinds of work. (`tokio::spawn` is short for `tokio::task::spawn`; same module.)

| | `tokio::spawn` | `tokio::task::spawn_blocking` |
|---|---|---|
| Accepts | a Future — `async { }` block / `async fn` call | a closure — `\|\| { }` (plain sync code) |
| Runs it on | the **worker pool** | the **blocking pool** (separate threads, created on demand, ≤512 by default) |
| Meant for | work that `.await`s: network, timers, channels — IO-bound | work that *cannot* `.await`: pure computation, sync IO, blocking C libraries (RocksDB!) |
| Interruptible | cooperatively — at `.await`s only | it's a thread — the kernel preempts it |
| Cost | ~hundreds of bytes (a task) | thread cost (pooled; cheap when warm) |

Usage, side by side:

```rust
// tokio::spawn — work that awaits:
let h = tokio::spawn(async {
    let data = socket.read(...).await;     // releases the worker while waiting ✓
    process_cheaply(data)                  // short CPU bits are fine (<~100 µs)
});

// spawn_blocking — work that blocks:
let h = tokio::task::spawn_blocking(|| {
    rocksdb_get(key)                       // blocks — but on the blocking pool, harmless
});

// results are collected the same way in both:
let result = h.await.unwrap();
```

**The decision rule is one question:** *does this code hit an `.await` regularly while it runs?* (Remember: `.await` is not "I'm done" — it is "I must wait for something; let others use the worker meanwhile"; the task pauses, it doesn't finish.) You ask the question of the code you're about to run, not of yourself:

```
Code hits .await regularly            Code runs long with no .await
(waits on network, timers, channels)  (pure computation, sync IO, a RocksDB call)
        │                                     │
        ▼                                     ▼
   tokio::spawn                         spawn_blocking
   (can share the workers politely —   (it will run long and uninterrupted anyway —
    it surrenders often)                so send it to a thread whose occupation
                                        is NOT a problem: the blocking pool)
```

The apparent contradiction is the design itself: code sent to `spawn_blocking` never awaits and runs long — **which is exactly why it goes there**. A blocking-pool thread's job is to be occupied; fairness there is the kernel's preemption. The worker pool lives by a culture of surrender — await-less code is a disaster there (4900 ms) and ordinary shift-work on the blocking pool.

Two subtleties worth pocketing: **(1) wrapping doesn't fool anyone** — `tokio::spawn(async { burn(5) })` gains no `.await` from the `async` wrapper, as the experiment proves; **(2) the opposite mistake exists too** — pushing *everything* to `spawn_blocking` brings thread costs back and forfeits async's whole economy (few threads, much work). The sound architecture: IO on the workers, CPU/blocking work on the blocking pool.

### Preemptive vs cooperative

The **kernel scheduler is preemptive**: a hardware timer fires an interrupt every few milliseconds; whatever a thread is doing, the kernel forcibly pauses it, saves its registers, and seats another thread. The thread is never asked. This is why 8 busy threads could share 2 vCPUs in Part 3 — an infinite loop cannot starve its neighbors, the kernel keeps taking the CPU back.

The **tokio scheduler is cooperative**: the runtime cannot interrupt a running task. A task releases its worker only when it reaches a surrender point in its own code — and that point is `.await`. A task that never reaches one keeps the worker forever.

> Kernel scheduler: police — pulls you over. tokio scheduler: a gentlemen's agreement — you must yield on your own.

### What `.await` actually does

An `async fn` does not run when called; it produces a **Future** — a pausable description of work. `.await` means: *"I need this result; if it is not ready, release the worker, and resume me here when it is."*

```rust
let n = socket.read(&mut buf).await;
```

Mechanically: if the data isn't there yet, the task is suspended *at that line* — its position and live variables are saved (the compiler turns the function into a state machine). The worker immediately picks up another task. When the data arrives, tokio marks the task ready, and some worker resumes it exactly where it stopped. `.await` is both a waiting point and **the gate where the worker is handed back** — it is the "cooperate" in cooperative scheduling.

The dark side follows directly: a loop with **no** `.await` — `loop { count += 1 }` — has no gate. Running a task is, for the worker thread, just a function call; if the function never reaches an `.await`, the worker executes it without pause. tokio is a library, not a kernel: it has no timer interrupt to force the issue. (The kernel still preempts the worker *thread* — but in favor of other processes; tokio's other *tasks* stay stuck in the queues.)

### The task queues and work-stealing

First, how a task is born: `tokio::spawn(async { ... })` is the task-world sibling of `thread::spawn` — but it creates no OS thread. It places a lightweight task (a few hundred bytes) into the runtime's queues and returns a `JoinHandle`; some worker will run it when its turn comes.

Where do ready-to-run tasks wait? In the runtime's **task queues** — tokio's counterpart of the kernel's run queue, except the entries are tasks, not threads, and the manager is tokio, not the kernel:

```
                 ┌──────────────────────────────┐
   tokio::spawn →│  global queue (entry gate)   │
                 └──────────────┬───────────────┘
                                ↓ distributed
        worker 0's local queue          worker 1's local queue
        [task C] [task D]               [task E]
              ↓                                ↓
        worker 0: running task A        worker 1: running task B
```

- Each worker owns a **local queue**; newly spawned tasks and tasks that just woke up (timer fired, IO ready) land in these queues.
- When a worker's current task suspends at an `.await`, the worker takes the next ready task **from its own local queue**.
- If its queue is empty, it **steals from another worker's queue** — *work-stealing*, tokio's load balancer: no worker idles while another has a backlog.

### The latency sensor: a heartbeat task

The experiments ahead need an instrument that feels scheduling delay *from the inside*. A **heartbeat task** is exactly this much:

```rust
loop {
    sleep(Duration::from_millis(100)).await;   // ASK to be woken 100 ms from now
    // on waking: what time is it REALLY — how late am I?
}
```

Everything happens on workers — tasks run nowhere else. The subtlety: during `sleep(...).await` the task does **not** sit on a worker; it is suspended, occupying nobody (the timer ticks in tokio's own agenda). When the 100 ms elapse, tokio marks the task ready, it enters a task queue — and waits for a worker. **That wait is the measurement**: planned wake-up T, actual run T+Δ. With idle workers, Δ ≈ 0; with workers held hostage by await-less tasks, Δ = the time spent in the queue. The clock is read by the task itself, on the worker, once it finally runs.

### "Don't block the event loop"

The number-one rule of every async runtime (Node.js, Python asyncio, tokio alike), and it follows from the three sections above: CPU-heavy or sync-blocking code inside an ordinary task occupies a worker without surrender points; every task in that worker's queue waits. tokio's guidance: between two `.await`s a task should run for roughly **10–100 µs, not more**. The classic production symptom: one endpoint does heavy parsing/compression (or a sync file read) in an async handler — and *every* connection on the server stalls at once.

The escape hatch is `tokio::task::spawn_blocking(closure)`: it moves the closure to a **separate blocking-thread pool** (real OS threads, spun up on demand — up to 512 by default), where the *kernel's preemptive* scheduler manages it — while the async workers stay free for tasks that do await. In short: CPU-bound work is deported from the cooperative world back to the preemptive world, which is built for it. Note where the fault lies in the horror story above: never in `tokio::spawn` itself — in handing await-less CPU work to it.

```
wrong:  2 workers ← occupied by 2 CPU-bound tasks  → the async world is locked
right:  2 workers ← free: heartbeats, IO, timers flow
        + blocking pool ← the CPU work lives here, preempted fairly by the kernel
```

The experiments that follow turn every claim above into numbers.

## Experiment 4.3 — how many workers does tokio start?

tokio's multi-thread runtime carries many lightweight tasks on a few OS **worker threads**. The sizing of that pool is exactly where Experiment 3.7 becomes practical: does tokio follow `available_parallelism()` — and therefore see cgroup quotas — or does it read the machine? Measured, not assumed.

The probe ([`tokioburn/src/bin/01_workers.rs`](tokioburn/src/bin/01_workers.rs)):

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

New concepts, line by line:

| Code | What it does |
|---|---|
| `Builder::new_multi_thread()...build()` | Constructs the tokio **runtime**: a worker-thread pool plus a task scheduler. The `#[tokio::main]` macro does this invisibly; we build it explicitly to inspect it. No worker count given — the default choice *is* the experiment. |
| `rt.metrics().num_workers()` | Asks the runtime itself how many worker threads it started — the witness inside the process. |
| `rt.block_on(async { ... })` | The gate between the sync world (`main`) and the async world: hands the runtime its first task and blocks `main` until that task completes. |
| `tokio::time::sleep(...).await` | tokio's sleep. `.await` means "while I wait, release the worker — other tasks may run." (`thread::sleep` would hold the worker hostage; that difference is Experiment 4.3.) Here it just keeps the process alive 15 s so it can be inspected from outside. |

The runs — bare, then inside a cgroup at 0.5 and 1.5 vCPU:

```bash
cargo build --release

# ── 1: bare ──
./target/release/01_workers
# while it sleeps, from a SECOND terminal, with the PID it printed:
ps -T -p <PID>                  # every thread of the process, with names (external witness)

# ── 2: caged, quota 0.5 vCPU ──
sudo mkdir /sys/fs/cgroup/lab
echo $$ | sudo tee /sys/fs/cgroup/lab/cgroup.procs
echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max
./target/release/01_workers

# ── 3: caged, quota 1.5 vCPU ──
echo "150000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max
./target/release/01_workers

# ── CLEANUP ──
echo $$ | sudo tee /sys/fs/cgroup/cgroup.procs
sudo rmdir /sys/fs/cgroup/lab
```

Real output (t3.large, bare):

```
$ ./target/release/01_workers
available_parallelism: Ok(2)
tokio workers: 2  (PID: 16590)

$ ps -T -p 16590
    PID    SPID TTY          TIME CMD
  16590   16590 pts/1    00:00:00 01_workers          ← main thread (parked in block_on)
  16590   16591 pts/1    00:00:00 tokio-rt-worker    ← worker 1
  16590   16592 pts/1    00:00:00 tokio-rt-worker    ← worker 2
```

Two details worth the look: tokio **names** its threads (`tokio-rt-worker`), which makes `ps -T`/`top -H` diagnosis in production pleasant; and the process holds 3 threads total — 1 main (blocked in `block_on`) + 2 workers.

The cgroup runs (same probe, caged shell):

```
$ echo "50000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max     # 0.5 vCPU
$ ./target/release/01_workers
available_parallelism: Ok(1)
tokio workers: 1  (PID: 16611)
$ ps -T -p 16611
  16611   16611  01_workers
  16611   16612  tokio-rt-worker          ← a single worker now

$ echo "150000 100000" | sudo tee /sys/fs/cgroup/lab/cpu.max    # 1.5 vCPU
$ ./target/release/01_workers
available_parallelism: Ok(1)
tokio workers: 1  (PID: 16841)
```

### Results

| Environment | `available_parallelism()` | tokio workers |
|---|---|---|
| bare | 2 | 2 |
| quota 0.5 vCPU | 1 | 1 |
| quota 1.5 vCPU | 1 | **1** — floor, again |

### What it teaches

1. **tokio sizes its pool from `available_parallelism()`** — so everything Experiment 3.7 established transfers: quota-aware, affinity-aware, floor semantics.
2. **"Quota-aware" means the k8s *limit*, never the *request*.** `limits: cpu` becomes `cpu.max`, which the runtime can read; `requests: cpu` becomes `cpu.weight` — a contention share from which no CPU count can even be derived. The runtime cannot see requests:

| Pod setting (64-vCPU node) | tokio workers |
|---|---|
| `limit: 500m` | 1 |
| `limit: 2` | 2 |
| `limit: 1500m` | 1 (floor) |
| **no limit, only `request: 2`** | **64** |

3. **The last row is the catch.** The "no limits for latency-critical services" strategy has a hidden cost: with no quota to read, the runtime falls back to the node's CPU count and starts 64 workers. On a quiet node that's free burst capacity; on a busy node, `cpu.weight` squeezes those 64 threads into a ~2-CPU share — run-queue crowding, §3.6's source-A stalls. The cure when running limitless: set the worker count explicitly (`Builder::worker_threads(n)` or the `TOKIO_WORKER_THREADS` env var), sized near your request.

## Experiment 4.4 — blocking the event loop, measured

Every claim of §4.2, turned into numbers. One probe, three modes: a **heartbeat task** that wants to wake every 100 ms and records its worst delay (Δ), sharing the runtime with two `.await`-less CPU-bound **burn tasks** — placed the wrong way (`tokio::spawn`), the right way (`spawn_blocking`), or not at all (control).

The probe is [`tokioburn/src/bin/02_heartbeat.rs`](tokioburn/src/bin/02_heartbeat.rs):

```rust
use std::time::{Duration, Instant};
use tokio::time::sleep;

const BATCH: u64 = 1_000_000;

fn burn(secs: u64) -> u64 {                    // 02_threads' counter — deliberately .await-less
    let start = Instant::now();
    let mut count: u64 = 0;
    while start.elapsed() < Duration::from_secs(secs) {
        for _ in 0..BATCH {
            count = std::hint::black_box(count + 1);
        }
    }
    count
}

#[tokio::main]                                  // macro form of 4.3's Builder: default runtime + block_on
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();

    let hb = tokio::spawn(async {               // the latency sensor of §4.2
        let mut worst = Duration::ZERO;
        for _ in 0..50 {                        // 50 × 100 ms ≈ 5 s
            let planned = Instant::now() + Duration::from_millis(100);
            sleep(Duration::from_millis(100)).await;
            let delta = planned.elapsed();      // Δ: how late past the planned wake-up?
            if delta > worst { worst = delta; }
        }
        worst
    });

    match mode.as_str() {
        "spawn"    => { for _ in 0..2 { tokio::spawn(async { burn(5) }); } }          // wrong way
        "blocking" => { for _ in 0..2 { tokio::task::spawn_blocking(|| burn(5)); } }  // right way
        _          => {}                                                             // control
    }

    let worst = hb.await.unwrap();              // a task's JoinHandle is .awaited, not joined
    println!("mode={:8}  worst heartbeat delay: {:.1} ms",
             if mode.is_empty() { "control" } else { &mode },
             worst.as_secs_f64() * 1000.0);
}
```

The runs (bare EC2 — no cgroup; this time the culprit is the code, not a quota):

```bash
cargo build --release
./target/release/02_heartbeat            # A: control
./target/release/02_heartbeat spawn      # B: wrong way
./target/release/02_heartbeat blocking   # C: right way
```

Real output (t3.large):

```
$ ./target/release/02_heartbeat
mode=control   worst heartbeat delay: 1.3 ms
$ ./target/release/02_heartbeat spawn
mode=spawn     worst heartbeat delay: 4900.1 ms
$ ./target/release/02_heartbeat blocking
mode=blocking  worst heartbeat delay: 2.5 ms
```

### Results

| Mode | Where the CPU work ran | Worst heartbeat Δ |
|---|---|---|
| control | nowhere (no burn) | 1.3 ms |
| `tokio::spawn` | on the 2 async workers | **4900.1 ms** |
| `spawn_blocking` | on the blocking pool | 2.5 ms |

### What it teaches

1. **4900 ms is not a slowdown — it is starvation.** The heartbeat asked to wake every 100 ms; with both workers held hostage it did not get its *first* turn until the burns finished (~4.9 s). Cooperative scheduling has no fairness rescue: a task that never `.await`s starves every task in the queues, completely, for its whole duration.
2. **Note what the kernel did and didn't do.** The kernel kept preempting the worker *threads* the whole time — other processes on the machine ran fine. But tokio's task queues are not the kernel's run queue: preemption one layer down does nothing for tasks stuck a layer up. Two schedulers are stacked; only the bottom one is preemptive.
3. **`spawn_blocking` is the cure, measured.** Same CPU work, same machine: Δ collapses from 4900 to 2.5 ms. The work moved to the blocking pool, where the kernel arbitrates preemptively; the async workers stayed free. (The small 2.5 vs 1.3 gap is honest too: the burn threads still compete with the workers for the machine's 2 vCPUs — preemption shares fairly, not freely.)
4. **Production translation.** This is the anatomy of "one endpoint made the whole API hang": a single CPU-heavy or sync-blocking handler on the worker pool delays *every* connection. And it is exactly why a storage engine's blocking calls (e.g. RocksDB reads/writes under a tokio server) belong behind `spawn_blocking` or a dedicated thread pool.

### The third way: cooperative yielding

If the rude loop's crime is never reaching an `.await`, there is a third fix: **teach it manners** — insert a surrender point into the loop itself. The probe is [`tokioburn/src/bin/03_heartbeat_v2.rs`](tokioburn/src/bin/03_heartbeat_v2.rs): same as 02, except `spawn` mode runs a *polite* burn:

```rust
async fn burn_sleepy(secs: u64) -> u64 {          // async now — see why below
    let start = Instant::now();
    let mut count: u64 = 0;
    while start.elapsed() < Duration::from_secs(secs) {
        for _ in 0..BATCH {
            count = std::hint::black_box(count + 1);
        }
        sleep(Duration::from_millis(1)).await;    // ← the one added line: surrender per batch
    }
    count
}
```

**Why the function had to become `async`:** `.await` is only legal inside an async fn — and not as a syntax whim. A plain `fn` compiles to straight-line code that runs to completion; it has no machinery to be paused. Marking it `async` makes the compiler rebuild the body as a **state machine**: every `.await` becomes a pause-station where the live locals (`count`, `start`…) are saved, so the function can be suspended and resumed there. The signature change is the *license to pause*. (The call site changes too: `burn_sleepy(5)` alone produces a Future — it runs only when awaited: `async { burn_sleepy(5).await }`.)

**Interlude — the measurement that measured nothing (wrong measurement #4).** The first version of this probe printed beautiful numbers: `spawn` at 2.0 ms — one batch's length, exactly what theory predicted. We almost believed it. Then a read of the code revealed the spawn arm said `tokio::spawn(async { burn_sleepy(5) })` — **no `.await`**. Calling an `async fn` produces a *lazy* Future — a plan; unawaited, the plan never runs. The burns never executed; "spawn" mode was a second control group, and the compiler never said a word. The fingerprint to remember: **when every mode measures the same as control, suspect that nobody is actually running** (a glance at `top` — ~0 % CPU — settles it). This is async Rust's most silent bug, met live. The fix is six characters:

```rust
tokio::spawn(async { burn_sleepy(5).await });    // .await — now the plan actually runs
```

With the fix in, two polite variants were measured — `sleep(1 ms)` per batch (03) and `yield_now()` per batch (04):

```
$ ./target/release/03_heartbeat_v2 spawn        # sleep(1ms) per batch
mode=spawn     worst heartbeat delay: 2.7 ms
$ ./target/release/04_heartbeat_v3 spawn        # yield_now() per batch
mode=spawn     worst heartbeat delay: 5.4 ms
```

The difference between the two surrender styles:

```
sleep(1ms).await   = "I release the worker AND do not wake me before 1 ms"
                      → task is 'not ready' for 1 ms — a deliberate loss of time

yield_now().await  = "I release the worker but I AM ready —
                      let the queue turn over, resume me on my turn"
                      → re-enters the queue ready; resumes almost
                        immediately if the queue is empty
```

All four ways, side by side (02 re-run the same day: control 2.7, spawn 4900.8, blocking 4.0 — numbers wobble a couple of ms between runs; the story doesn't):

| Way | Where the CPU work sits | Worst heartbeat Δ | Burn's own cost |
|---|---|---|---|
| rude `tokio::spawn` (02) | workers, no surrender | **4900.8 ms** | none — but lethal to neighbors |
| polite — `sleep(1ms)` per batch (03) | workers, rests every batch | **2.7 ms** | ~1 ms idle per ~2 ms batch (≈33 % self-tax) |
| polite — `yield_now()` per batch (04) | workers, surrenders but never rests | **5.4 ms** | ~none |
| `spawn_blocking` (02) | blocking pool | 4.0 ms | none (thread cost instead) |

**Two lessons in the numbers:**

1. **Politeness works, and its bound is the batch.** Both polite variants collapse 4900 ms to single-digit ms. The residual delay ≈ the distance between `.await`s (one or two ~2 ms batches) — tokio's 10–100 µs guidance seen from the victim's side: *your inter-await gap is someone else's worst-case latency.*
2. **The surprise: `yield_now` (5.4) is worse for the neighbor than `sleep` (2.7) — nothing is free.** Sleepy burns rest 1 ms every cycle, so the workers are often idle at the moment the heartbeat wakes — it seats instantly. `yield_now` burns never rest: the workers stay 100 % occupied and the waking heartbeat always waits out the current batch. `sleep` buys neighbor latency with its own throughput (~33 % tax); `yield_now` keeps its throughput and bills a couple of ms to the neighbors. Pick by what the service must protect.

Practical hierarchy stands: **chunkable short CPU work → yield (or micro-sleep) per chunk; long, foreign, or sync work (RocksDB) → `spawn_blocking`** — you cannot sprinkle `.await`s into a C library's code.

### Problem → approach → options (4.4)

**The real-world problem this maps to:** a service's p99 explodes, or the whole API freezes for seconds at a time — typically correlated with one particular endpoint or job. Signature symptoms: *all* requests stall simultaneously (not just the heavy one); overall CPU may even look low while one or two cores are pegged.

**How to approach it:** suspect await-less stretches on the worker pool. Confirm before fixing: a heartbeat probe like this experiment's (or tokio-console / runtime metrics in production) shows scheduling delay; `top -H` shows which threads are pegged — if it's the `tokio-rt-worker`s while traffic stalls, the diagnosis is made.

**The options, ranked:**

| Option | When | Cost |
|---|---|---|
| `spawn_blocking` | long, sync, or foreign code (RocksDB, compression libs, big file IO) | thread-pool usage; result comes back via `.await` |
| chunk + `yield_now()` | CPU work you own and can split into short pieces | a couple of ms neighbor latency per chunk; code discipline |
| chunk + micro-`sleep` | same, when you'd rather idle the workers between chunks | self-throughput tax (~33 % in our numbers) |
| dedicated compute pool (e.g. rayon) + channel | heavy, sustained, parallel computation | architecture complexity — right for compute-centric services |
| ~~more workers~~ | never as the fix | more lanes that the same code will clog |

## Experiment 4.5 — a million waiting tasks on two threads

Experiment 4.4 showed async's weak flank (non-awaiting work). This one measures its reason to exist: **how many *waiting* tasks can two worker threads carry?** The probe is the heartbeat pattern multiplied: N tasks, each waking every 100 ms for 50 rounds, each tracking its own worst wake-up delay; `main` reports the worst across all of them. `sleep` here is the lab model of a network wait — both mean "suspended, occupying nobody, wake me on an event." N waiting tasks ≈ N idle connections on a server.

The probe is [`tokioburn/src/bin/05_ioload.rs`](tokioburn/src/bin/05_ioload.rs):

```rust
use std::time::{Duration, Instant};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let tasks: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000);
    println!("PID: {}", std::process::id());

    let start = Instant::now();
    let handles: Vec<_> = (0..tasks).map(|_| tokio::spawn(async {
        let mut worst = Duration::ZERO;
        for _ in 0..50 {                                  // each task = a 50-round heartbeat
            let planned = Instant::now() + Duration::from_millis(100);
            sleep(Duration::from_millis(100)).await;
            let delta = planned.elapsed();
            if delta > worst { worst = delta; }
        }
        worst
    })).collect();

    let mut worst_all = Duration::ZERO;                   // worst across ALL tasks
    for h in handles {
        let w = h.await.unwrap();
        if w > worst_all { worst_all = w; }
    }
    println!("{tasks} task(s): finished in {:.2} s, worst wake-up delay: {:.1} ms",
             start.elapsed().as_secs_f64(),
             worst_all.as_secs_f64() * 1000.0);
}
```

Runs (bare EC2), with `ps -T -p <PID>` from a second terminal during each:

```
$ ./target/release/05_ioload 1000
1000 task(s): finished in 5.07 s, worst wake-up delay: 2.6 ms
$ ./target/release/05_ioload 10000
10000 task(s): finished in 5.09 s, worst wake-up delay: 4.4 ms
$ ./target/release/05_ioload 100000
100000 task(s): finished in 5.43 s, worst wake-up delay: 32.1 ms
$ ./target/release/05_ioload 1000000
1000000 task(s): finished in 42.39 s, worst wake-up delay: 1067.6 ms

$ ps -T -p <PID>          # identical at every scale:
  05_ioload
  tokio-rt-worker
  tokio-rt-worker          ← 3 threads total, whether 1 000 or 1 000 000 tasks
```

### Results

| Tasks | Duration | Worst wake-up delay | OS threads | Wake-ups needed/s |
|---|---|---|---|---|
| 1 000 | 5.07 s | 2.6 ms | **3** | 10 k |
| 10 000 | 5.09 s | 4.4 ms | **3** | 100 k |
| 100 000 | 5.43 s | 32.1 ms | **3** | 1 M |
| 1 000 000 | **42.39 s** | **1067.6 ms** | **3** | 10 M — past the ceiling |

### What it teaches

1. **The async economy, proven.** 100 000 concurrently waiting jobs on 2 worker threads, worst wake-up 32 ms. As OS threads this would be ~100 000 stacks (hundreds of GB of address space, kernel run-queue chaos); here the external witness never counted past 3 threads. This is why network servers are written async: waiting is nearly free.
2. **Nearly free — not infinitely free.** Each task wakes 10×/s, so 1 M tasks demand **10 million wake-ups per second** — and every wake-up costs the workers a few µs of bookkeeping (timer pop, queue push, poll). That bill exceeded 2 vCPUs: the run stretched 5 s → 42 s and the worst delay hit a full second. The ceiling for *waiting* concurrency is ~1000× higher than for threads, but it is still priced in the same currency: **CPU time**. Every layer of this lab ends at the same resource.
3. **Sizing corollary:** "how many connections can this pod hold?" is not a memory question first — it is a *wake-rate* question: events/second × cost-per-event vs the pod's CPU limit. That formula carries straight into Part 6.

### Problem → approach → options (4.5)

**The real-world problem this maps to:** capacity planning for connection-heavy services — "can this pod hold 100 k websockets / MQTT clients / idle keep-alive connections?" And its failure mode: a service that was fine at 50 k connections collapses at 500 k with soaring latency, while memory looks healthy — because the bottleneck was never memory.

**How to approach it:** budget in *events per second*, not connections: `connections × wake-ups per connection per second × CPU cost per wake-up` must fit inside the pod's CPU allowance. Measure the per-event cost with a model like this probe rather than guessing it.

**The options, ranked:**

| Option | Effect |
|---|---|
| lower per-connection wake frequency (longer heartbeat/keep-alive intervals) | divides the event rate directly — usually the cheapest win |
| batch events (one timer serving many connections, coalesced writes) | cuts bookkeeping cost per logical event |
| scale the CPU limit with event rate | honest but costs money; the formula says how much |
| shard connections across pods | horizontal version of the same formula |
| ~~add RAM~~ | not the bottleneck for waiting tasks — measure before buying |

[↑ Go back to TOC](#table-of-contents)

# Part 5 — Kubernetes requests & limits *(coming soon)*

The mechanism, end to end: the Part 3 burners deployed as pods on OpenShift (a static musl binary in a `FROM scratch` image), with the experiment matrix replayed in YAML. Walking the cgroup tree that kubelet builds (`kubepods.slice/...`), mapping `requests`/`limits` to the exact files from Part 2, confirming that the cell we measured by hand (`echo "50000 100000" > cpu.max` → 241 M iter/s) is the same cell Kubernetes builds from `limits: cpu: 500m`, and judging each request/limit combination as *helpful, harmful, or neutral* for each workload type.

[↑ Go back to TOC](#table-of-contents)

# Part 6 — Performance lab: sizing Redis & Dragonfly *(coming soon)*

The payoff chapter: real engines, real load, measured sizing recipes — on the VM with hand-set cgroups, and on OpenShift with requests/limits. Two engines with opposite architectures — Redis (single-threaded event loop ≈ our 1-thread row) and Dragonfly (thread-per-core ≈ our 8-thread row) — put under the same load while their CPU constraints are swept. Two instruments, cross-checking each other:

- **Type A:** our own tokio RESP client from Part 4 — models the *application's actual write path*, workload shape fully under our control.
- **Type B:** `memtier_benchmark` (Redis Ltd.'s standard benchmark image) — the industry reference the world trusts.

### Test topology: the measurer must never starve

A load generator that runs out of CPU reports its own agony as the server's latency. The client therefore always lives on separate hardware from the server, in both environments:

```
VM scenario (cgroup by hand):          OpenShift scenario (requests/limits):

  VM 1                 VM 2              node 1                node 2
┌─────────────┐     ┌──────────────┐   ┌─────────────┐     ┌──────────────┐
│ RESP client │ ──> │ redis /      │   │ client pod  │ ──> │ server pod   │
│ / memtier   │     │ dragonfly    │   │ (generous   │     │ (requests/   │
│ (no limits) │     │ (cpu.max     │   │  resources, │     │  limits under│
│             │     │  swept)      │   │  no limits) │     │  test)       │
└─────────────┘     └──────────────┘   └─────────────┘     └──────────────┘
```

Constants and variables, strictly separated: the network path stays fixed (same VM pair / same node pair, same AZ), the client stays unconstrained, and **the only thing that changes between runs is the server's CPU constraint**. Every run records the same triple: client-side p99, client-side throughput, server-side `cpu.stat` / `container_cpu_cfs_throttled_periods_total`. The correlation between the last one and the first one is the lab's signature move.

Deliverable: measured sizing recipes — "for this workload shape, give Redis *this* request/limit and Dragonfly *that* one, and here are the numbers that prove it."

[↑ Go back to TOC](#table-of-contents)
