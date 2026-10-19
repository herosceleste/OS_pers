#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="${1:-/mnt/c/Elyon_VM/elyon-serial.log}"
EXPECTED_BUILD_ID="${2:-$(git -C "$ROOT/.." rev-parse --short=12 HEAD)}"

[[ -f "$LOG" ]] || {
  echo "[FAIL] log VMware introuvable: $LOG" >&2
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

require_regex "\[acpi\] cpu_count=4 usable=4 ioapics=[1-9][0-9]* isos=[0-9]+ nmis=[0-9]+"   "VMware n'expose pas 4 CPU utilisables dans MADT"

require_fixed "ELYON_ACPI_READY" "ACPI non validé"
require_fixed "ELYON_LAPIC_READY" "Local APIC non validé"
require_fixed "ELYON_PERCPU_READY" "état per-CPU non validé"

mapfile -t PREPARED < <(
  grep -E '^\[percpu\] cpu=[1-9][0-9]* apic_id=[0-9]+ state=prepared ' "$LOG"
)

if [[ "${#PREPARED[@]}" -ne 3 ]]; then
  echo "[FAIL] 3 AP préparés attendus dans VMware, obtenus: ${#PREPARED[@]}" >&2
  exit 1
fi

for line in "${PREPARED[@]}"; do
  cpu="$(sed -nE 's/^\[percpu\] cpu=([0-9]+).*/\1/p' <<<"$line")"
  apic="$(sed -nE 's/^\[percpu\].* apic_id=([0-9]+).*/\1/p' <<<"$line")"
  require_fixed "ELYON_AP_READY cpu=$cpu apic_id=$apic"     "AP cpu=$cpu apic_id=$apic n'a pas atteint READY sous VMware"
done

require_regex "\[smp\] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8"   "tous les AP ne sont pas online"
require_regex "\[smp\] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10"   "stabilité SMP non confirmée"
require_fixed "ELYON_SMP_READY" "marqueur SMP ready absent"
require_fixed "ELYON_SMP_STABLE" "marqueur SMP stable absent"

require_fixed "[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0"   "changement de contexte K003-11 non confirmé"
require_fixed "ELYON_CONTEXT_SWITCH_READY" "marqueur context-switch absent"

require_regex "\[ioapic\] legacy_routes=[1-9][0-9]* overrides_applied=[0-9]+ shadowed=[0-9]+ id_mismatches=[0-9]+ masked=([1-9][0-9]*)/\1 all_masked=1"   "IOAPIC final non maîtrisé"
require_fixed "ELYON_IOAPIC_READY" "marqueur IOAPIC absent"

require_regex "\[timer\] ticks=(1[0-9][0-9]|[2-9][0-9]{2,}) eoi_failures=0 periodic=1"   "timer LAPIC non confirmé"
require_fixed "ELYON_TIMER_READY" "marqueur timer absent"

require_fixed "ELYON_KERNEL_READY_0_0_3" "boot nominal final absent"

if grep -Eq '\[KERNEL PANIC\]|\[EXCEPTION\]|\[[^]]+\] FAILED|ELYON_[A-Z_]+_FAILED' "$LOG"; then
  echo "[FAIL] anomalie inattendue détectée dans le log VMware" >&2
  grep -E '\[KERNEL PANIC\]|\[EXCEPTION\]|\[[^]]+\] FAILED|ELYON_[A-Z_]+_FAILED' "$LOG" >&2 || true
  exit 1
fi

echo "[PASS] VMware 4 vCPU: Elyon 0.0.3 K003-01..12 validés sur build_id=$EXPECTED_BUILD_ID"
