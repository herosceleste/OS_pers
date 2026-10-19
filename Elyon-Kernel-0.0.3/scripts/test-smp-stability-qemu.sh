#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-smp-stability.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-35}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

for cpu in 1 2 3; do
  grep -Fq "ELYON_AP_IDLE cpu=$cpu apic_id=$cpu" "$LOG" || {
    echo "[FAIL] AP cpu=$cpu n'a pas atteint l'état idle SMP" >&2
    exit 1
  }

  grep -Fq "[smp] stable cpu=$cpu apic_id=$cpu state=online heartbeat=10 boot_count=1" "$LOG" || {
    echo "[FAIL] AP cpu=$cpu n'a pas conservé un état online stable avec heartbeat et boot_count=1" >&2
    exit 1
  }
done

STABLE_COUNT="$(grep -c '^\[smp\] stable cpu=' "$LOG" || true)"
if [[ "$STABLE_COUNT" -ne 3 ]]; then
  echo "[FAIL] 3 AP stables attendus, obtenus: $STABLE_COUNT" >&2
  exit 1
fi

grep -Fq "[smp] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10" "$LOG" || {
  echo "[FAIL] fenêtre de stabilité SMP 10 rounds non prouvée" >&2
  exit 1
}

grep -Fq "ELYON_SMP_STABLE" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SMP_STABLE absent" >&2
  exit 1
}

if grep -Eq "\[smp\] (AP FAILED|STABILITY FAILED)" "$LOG"; then
  echo "[FAIL] une erreur SMP a été signalée" >&2
  exit 1
fi

grep -Fq "ELYON_KERNEL_READY_0_0_3" "$LOG" || {
  echo "[FAIL] BSP instable après la fenêtre SMP (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-10 BSP stable, 3 AP idle/réactifs, 10 heartbeats chacun, aucun redémarrage"
