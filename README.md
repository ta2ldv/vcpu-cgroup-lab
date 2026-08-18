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

# Part 3 — Rust load generator *(coming soon)*

A self-measuring load generator: N threads, each counting work done per second. Planned experiments: thread-count sweep vs 2 vCPUs (the parallelism wall), the same sweep inside a throttled cgroup, and `std::thread::available_parallelism()` vs reality.

# Part 4 — Kubernetes requests & limits *(coming soon)*

Walking the cgroup tree that kubelet builds (`kubepods.slice/...`), mapping `requests`/`limits` to the exact files from Part 2, observing CFS throttling on real pods on OpenShift, and why runtime thread-pool sizing needs to be cgroup-aware.
