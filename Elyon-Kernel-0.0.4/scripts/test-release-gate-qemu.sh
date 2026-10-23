#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-release-0.0.4.log"
EVIDENCE="$ROOT/build/qualification-0.0.4.txt"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-40}"

bash "$ROOT/scripts/build.sh"

BUILD_ID="$(git -C "$ROOT/.." rev-parse --short=12 HEAD)"
RUST_VERSION="$(rustc +1.99.1 --version)"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

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

require_fixed "ELYON_ACPI_READY" "K003-01/02/03: ACPI ready absent"
require_fixed "ELYON_LAPIC_READY" "K003-04: Local APIC ready absent"
require_fixed "ELYON_PERCPU_READY" "K003-07: per-CPU ready absent"
require_fixed "ELYON_AP_BOOT_READY" "K003-08: AP boot ready absent"
require_fixed "ELYON_SMP_READY" "K003-09: SMP ready absent"
require_fixed "ELYON_SMP_STABLE" "K003-10: SMP stable absent"
require_fixed "ELYON_CONTEXT_SWITCH_READY" "K003-11: context switch ready absent"
require_fixed "ELYON_IOAPIC_READY" "K003-05: IOAPIC ready absent"
require_fixed "ELYON_TIMER_READY" "K003-06: timer ready absent"
require_fixed "ELYON_KERNEL_READY_0_0_4" "boot nominal final absent"

require_regex "\[acpi\] RSDP=OK revision=[0-9]+ root=(XSDT|RSDT) address=0x[0-9a-f]+"   "K003-01: RSDP validé absent"
require_regex "\[acpi\] root=(XSDT|RSDT) entries=[1-9][0-9]* validated=[1-9][0-9]*"   "K003-02: racine ACPI validée absente"
require_regex "\[acpi\] cpu_count=4 usable=4 ioapics=[1-9][0-9]* isos=[0-9]+ nmis=[0-9]+"   "K003-03: topologie MADT 4 CPU absente"

require_fixed "[apic] EOI primitive=OK" "K003-04: EOI LAPIC absent"
require_regex "\[ioapic\] legacy_routes=[1-9][0-9]* overrides_applied=[0-9]+ shadowed=[0-9]+ id_mismatches=[0-9]+ masked=([1-9][0-9]*)/\1 all_masked=1"   "K003-05: IOAPIC non entièrement masqué"

require_regex "\[percpu\] cpus=4 capacity=64 bsp_logical=0 bsp_apic_id=[0-9]+ unique=1 owned=1 tr_loaded=1"   "K003-07: état per-CPU final invalide"

mapfile -t PREPARED < <(
  grep -E '^\[percpu\] cpu=[1-9][0-9]* apic_id=[0-9]+ state=prepared ' "$LOG"
)
if [[ "${#PREPARED[@]}" -ne 3 ]]; then
  echo "[FAIL] K003-07/09: 3 AP préparés attendus, obtenus: ${#PREPARED[@]}" >&2
  exit 1
fi

for line in "${PREPARED[@]}"; do
  cpu="$(sed -nE 's/^\[percpu\] cpu=([0-9]+).*/\1/p' <<<"$line")"
  apic="$(sed -nE 's/^\[percpu\].* apic_id=([0-9]+).*/\1/p' <<<"$line")"
  require_fixed "ELYON_AP_READY cpu=$cpu apic_id=$apic"     "K003-09: AP cpu=$cpu apic_id=$apic n'a pas atteint READY"
done

require_fixed "[smp] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8"   "K003-09: état SMP 4/4 absent"
require_fixed "[smp] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10"   "K003-10: fenêtre de stabilité SMP invalide"
require_fixed "[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0"   "K003-11: changement de contexte final invalide"
require_regex "\[timer\] ticks=(1[0-9][0-9]|[2-9][0-9]{2,}) eoi_failures=0 periodic=1"   "K003-06: timer périodique invalide"

if grep -Eq '\[KERNEL PANIC\]|\[EXCEPTION\]|\[[^]]+\] FAILED|ELYON_[A-Z_]+_FAILED' "$LOG"; then
  echo "[FAIL] anomalie fail-closed/panic inattendue dans le boot nominal" >&2
  grep -E '\[KERNEL PANIC\]|\[EXCEPTION\]|\[[^]]+\] FAILED|ELYON_[A-Z_]+_FAILED' "$LOG" >&2 || true
  exit 1
fi

{
  echo "ELYON KERNEL 0.0.4 - K003 REGRESSION BASELINE"
  echo "build_id=$BUILD_ID"
  echo "rust=$RUST_VERSION"
  echo "qemu_cpus=4"
  echo "qemu_rc=$RC"
  echo
  echo "SHA256:"
  cat "$ROOT/build/SHA256SUMS"
  echo
  echo "Markers:"
  grep -E '^ELYON_(ACPI_READY|LAPIC_READY|PERCPU_READY|AP_READY|AP_BOOT_READY|SMP_READY|SMP_STABLE|CONTEXT_SWITCH_READY|IOAPIC_READY|TIMER_READY|KERNEL_READY_0_0_4)' "$LOG" || true
  echo
  echo "Final summaries:"
  grep -E '^\[smp\] all_ready|^\[smp\] stability|^\[context\] result|^\[ioapic\] legacy_routes=|^\[timer\] ticks=' "$LOG" || true
} >"$EVIDENCE"

cat "$EVIDENCE"

echo "[PASS] Elyon 0.0.4 baseline: régressions K003-01..12 cohérentes sur 4 vCPU, boot nominal sans anomalie"
