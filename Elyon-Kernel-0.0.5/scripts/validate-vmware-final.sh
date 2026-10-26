#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="${1:-/mnt/c/Elyon_VM/elyon-serial.log}"
EXPECTED_BUILD_ID="${2:-$(git -C "$ROOT/.." rev-parse --short=12 HEAD)}"
# Hérité de K004-12 : le même gate qualifie le boot QEMU/OVMF (test-release-gate-qemu.sh)
# et le boot VMware ; seul le nom de l'environnement change dans les messages.
ENV_NAME="${ELYON_ENV:-VMware}"

[[ -f "$LOG" ]] || {
  echo "[FAIL] log $ENV_NAME introuvable: $LOG" >&2
  exit 1
}

require_fixed() {
  local marker="$1"
  local message="$2"
  grep -Fq "$marker" "$LOG" || {
    echo "[FAIL] $message" >&2
    exit 1
  }
}

require_regex() {
  local pattern="$1"
  local message="$2"
  grep -Eq "$pattern" "$LOG" || {
    echo "[FAIL] $message" >&2
    exit 1
  }
}

require_fixed "[kernel] build_id=$EXPECTED_BUILD_ID"   "le log ne correspond pas au commit local attendu $EXPECTED_BUILD_ID"

require_regex "\[acpi\] cpu_count=4 usable=4 ioapics=[1-9][0-9]* isos=[0-9]+ nmis=[0-9]+"   "$ENV_NAME n'expose pas 4 CPU utilisables dans MADT"

require_fixed "ELYON_ACPI_READY" "ACPI non validé"
require_fixed "ELYON_LAPIC_READY" "Local APIC non validé"
require_fixed "ELYON_PERCPU_READY" "état per-CPU non validé"

mapfile -t PREPARED < <(
  grep -E '^\[percpu\] cpu=[1-9][0-9]* apic_id=[0-9]+ state=prepared ' "$LOG"
)

if [[ "${#PREPARED[@]}" -ne 3 ]]; then
  echo "[FAIL] 3 AP préparés attendus sous $ENV_NAME, obtenus: ${#PREPARED[@]}" >&2
  exit 1
fi

for line in "${PREPARED[@]}"; do
  cpu="$(sed -nE 's/^\[percpu\] cpu=([0-9]+).*/\1/p' <<<"$line")"
  apic="$(sed -nE 's/^\[percpu\].* apic_id=([0-9]+).*/\1/p' <<<"$line")"
  require_fixed "ELYON_AP_READY cpu=$cpu apic_id=$apic"     "AP cpu=$cpu apic_id=$apic n'a pas atteint READY sous $ENV_NAME"
done

require_regex "\[smp\] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8"   "tous les AP ne sont pas online"
require_regex "\[smp\] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10"   "stabilité SMP non confirmée"
require_fixed "ELYON_SMP_READY" "marqueur SMP ready absent"
require_fixed "ELYON_SMP_STABLE" "marqueur SMP stable absent"

require_fixed "[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0"   "changement de contexte K003-11 non confirmé"
require_fixed "ELYON_CONTEXT_SWITCH_READY" "marqueur context-switch absent"

require_regex "\[task\] ids=[1-9][0-9]*,[1-9][0-9]* unique=1 capacity=256 created=2 transitions=12 invalid_rejected=2 states_seen=0x1f"   "K004-01: modèle de tâche, IDs ou transitions invalides"
require_fixed "[task] states=READY,RUNNING,BLOCKED,SLEEPING,DEAD affinity_enforced=1"   "K004-01: états de tâche ou affinité non qualifiés"
require_fixed "ELYON_TASK_MODEL_READY" "K004-01: marqueur task model absent"

mapfile -t TASK_STACKS < <(
  tr -d '\r' <"$LOG" | grep -E '^\[task-stack\] task=[0-9]+ stack=0x[0-9a-f]+\.\.0x[0-9a-f]+ size=64 KiB initial_rsp=0x[0-9a-f]+ observed_rsp=0x[0-9a-f]+ saved_rsp=0x[0-9a-f]+ arg=0x[0-9a-f]+ canary=OK$'
)
if [[ "${#TASK_STACKS[@]}" -ne 3 ]]; then
  echo "[FAIL] K004-02: 3 tâches avec pile dédiée attendues sous $ENV_NAME, obtenues: ${#TASK_STACKS[@]}" >&2
  exit 1
fi
require_fixed "[task-stack] tasks=3 distinct=1 overlaps=0 owned=3 rounds=2 switches=12 args_ok=3 canaries_ok=3"   "K004-02: synthèse piles/contextes invalide"
require_fixed "ELYON_TASK_STACKS_READY" "K004-02: marqueur task stacks absent"

require_regex "^\[sched\] tasks=([0-9]+),([0-9]+),([0-9]+) trace=\1,\2,\3,\1,\2,\3,\1,\2,\3,\1,\2,\3\s*$"   "K004-03: trace Round-Robin stricte absente"
require_fixed "[sched] policy=round-robin cooperative iterations=4 yields=9 exits=3 task_switches=11 bsp_switches=2 running_max=1 args_ok=3 rsp_ok=12 canaries_ok=3 queue_empty=1"   "K004-03: synthèse scheduler invalide"
require_fixed "ELYON_SCHED_RR_READY" "K004-03: marqueur scheduler absent"

require_regex "\[ioapic\] legacy_routes=[1-9][0-9]* overrides_applied=[0-9]+ shadowed=[0-9]+ id_mismatches=[0-9]+ masked=([1-9][0-9]*)/\1 all_masked=1"   "IOAPIC final non maîtrisé"
require_fixed "ELYON_IOAPIC_READY" "marqueur IOAPIC absent"

require_regex "\[timer\] ticks=(1[0-9][0-9]|[2-9][0-9]{2,}) eoi_failures=0 periodic=1"   "timer LAPIC non confirmé"
require_fixed "ELYON_TIMER_READY" "marqueur timer absent"

require_regex "^\[preempt\] tasks=([0-9]+),([0-9]+),([0-9]+) trace=(\1,\2,\3,){10}\1\s*$"   "K004-04: trace de préemption stricte (31 tranches) absente"
require_regex "^\[preempt\] policy=timer-preemptive quantum_ticks=1 target=30 preemptions=(3[0-9]|[4-9][0-9]|[1-9][0-9]{2,}) task_ticks=(3[0-9]|[4-9][0-9]|[1-9][0-9]{2,}) min_slices=(1[0-9]|[2-9][0-9]) counters=[1-9][0-9]*,[1-9][0-9]*,[1-9][0-9]* exits=3 running_max=1 eoi_failures=0 canaries_ok=3\s*$"   "K004-04: synthèse de préemption invalide"
require_fixed "ELYON_SCHED_PREEMPT_READY" "K004-04: marqueur préemption absent"

if grep -Fq "[context-integrity] FAILED" "$LOG"; then
  echo "[FAIL] K004-05: corruption de contexte détectée sous $ENV_NAME" >&2
  grep -F "[context-integrity] FAILED" "$LOG" >&2
  exit 1
fi
require_regex "^\[context-integrity\] tasks=[0-9]+,[0-9]+,[0-9]+ registers=15 df=1 preemptions=(3[0-9]|[4-9][0-9]|[1-9][0-9]{2,}) min_slices=(1[0-9]|[2-9][0-9]) busy_preemptions=(1[0-9]|[2-9][0-9]|[1-9][0-9]{2,}),(1[0-9]|[2-9][0-9]|[1-9][0-9]{2,}),(1[0-9]|[2-9][0-9]|[1-9][0-9]{2,}) iterations=[1-9][0-9]*,[1-9][0-9]*,[1-9][0-9]* corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3\s*$"   "K004-05: synthèse d'intégrité du contexte invalide"
require_fixed "ELYON_SCHED_CONTEXT_READY" "K004-05: marqueur contexte absent"

if grep -Eq '\[(fpu|fpu-integrity)\] FAILED' "$LOG"; then
  echo "[FAIL] K004-06: échec FPU ou corruption SIMD détectée sous $ENV_NAME" >&2
  grep -E '\[(fpu|fpu-integrity)\] FAILED' "$LOG" >&2
  exit 1
fi
require_regex "^\[fpu\] fxsr=1 sse2=1 xsave=[01] avx=[01] xcr0_supported=0x[0-9a-f]+ strategy=(xsave xcr0=0x[37] avx_enabled=[01] area=[0-9]+ align=64|fxsave xcr0=0x0 avx_enabled=0 area=512 align=16) forced_fxsave=0 "   "K004-06: configuration FPU invalide"
require_regex "^\[fpu-integrity\] tasks=[0-9]+,[0-9]+,[0-9]+ strategy=(xsave|fxsave) avx=(1 xmm=16 ymm_high=16|0 xmm=16 ymm_high=0) mxcsr=1 preemptions=(3[0-9]|[4-9][0-9]|[1-9][0-9]{2,}) min_slices=(1[0-9]|[2-9][0-9]) busy_preemptions=(1[0-9]|[2-9][0-9]|[1-9][0-9]{2,}),(1[0-9]|[2-9][0-9]|[1-9][0-9]{2,}),(1[0-9]|[2-9][0-9]|[1-9][0-9]{2,}) iterations=[1-9][0-9]*,[1-9][0-9]*,[1-9][0-9]* corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3\s*$"   "K004-06: synthèse d'isolation SIMD invalide"
require_fixed "ELYON_SCHED_FPU_READY" "K004-06: marqueur FPU absent"

# K004-07 : même contrôle indépendant que le test QEMU, appliqué au log VMware.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-percpu-qemu.sh" >/dev/null || {
  echo "[FAIL] K004-07: scheduler per-CPU non conforme sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-percpu-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K004-08 : même contrôle indépendant que le test QEMU, appliqué au log VMware.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-ipi-qemu.sh" >/dev/null || {
  echo "[FAIL] K004-08: rescheduling à distance non conforme sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-ipi-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K004-09 : même contrôle indépendant que le test QEMU, appliqué au log VMware.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-sleep-qemu.sh" >/dev/null || {
  echo "[FAIL] K004-09: sleep/wakeup non conforme sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-sleep-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K004-10 : même contrôle indépendant que le test QEMU, appliqué au log VMware.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-sync-qemu.sh" >/dev/null || {
  echo "[FAIL] K004-10: synchronisation non conforme sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-sync-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K004-11 : même contrôle indépendant que le test QEMU, appliqué au log VMware.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-stress-qemu.sh" >/dev/null || {
  echo "[FAIL] K004-11: stress SMP non conforme sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-sched-stress-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K005-01 : même contrôle indépendant que le test QEMU, appliqué au log.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-segments-qemu.sh" >/dev/null || {
  echo "[FAIL] K005-01: segments ou RSP0 non conformes sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-segments-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K005-02 : même contrôle indépendant que le test QEMU, appliqué au log.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-ring3-swapgs-qemu.sh" >/dev/null || {
  echo "[FAIL] K005-02: swapgs ou sonde ring 3 non conformes sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-ring3-swapgs-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K005-03 : même contrôle indépendant que le test QEMU, appliqué au log.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-paranoid-qemu.sh" >/dev/null || {
  echo "[FAIL] K005-03: entrée paranoïaque NMI/#MC non conforme sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-paranoid-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K005-04 : même contrôle indépendant que le test QEMU, appliqué au log.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-user-access-qemu.sh" >/dev/null || {
  echo "[FAIL] K005-04: SMEP/SMAP ou copies utilisateur non conformes sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-user-access-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

# K005-05 : même contrôle indépendant que le test QEMU, appliqué au log.
ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-user-pages-qemu.sh" >/dev/null || {
  echo "[FAIL] K005-05: pages utilisateur ou alias identité non conformes sous $ENV_NAME" >&2
  ELYON_CHECK_LOG="$LOG" ELYON_CPUS=4 bash "$ROOT/scripts/test-user-pages-qemu.sh" 2>&1 >/dev/null | head -3 >&2
  exit 1
}

require_fixed "ELYON_KERNEL_READY_0_0_5" "boot nominal final absent"

if grep -Eq '\[KERNEL PANIC\]|\[EXCEPTION\]|\[[^]]+\] FAILED|ELYON_[A-Z_]+_FAILED' "$LOG"; then
  echo "[FAIL] anomalie inattendue détectée dans le log $ENV_NAME" >&2
  grep -E '\[KERNEL PANIC\]|\[EXCEPTION\]|\[[^]]+\] FAILED|ELYON_[A-Z_]+_FAILED' "$LOG" >&2 || true
  exit 1
fi

echo "[PASS] $ENV_NAME 4 vCPU: Elyon 0.0.5 K003-01..12, K004-01..11 et K005-01..05 validés sur build_id=$EXPECTED_BUILD_ID"
