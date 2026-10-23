#!/usr/bin/env bash
set -euo pipefail

# K004-06 : isolation x87/SSE/AVX par tâche sous préemption.
# Avec ELYON_FPU_FORCE_FXSAVE défini, le noyau est construit avec le repli
# FXSAVE et le test exige ce chemin ; sinon il exige XSAVE si le CPU l'offre.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${ELYON_FPU_FORCE_FXSAVE:+fxsave}"
MODE="${MODE:-default}"
LOG="$ROOT/build/qemu-sched-fpu-$MODE.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-60}"
TARGET=30
MIN_BUSY_PREEMPTIONS=10

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Fq "ELYON_SCHED_CONTEXT_READY" "$LOG" || {
  echo "[FAIL] régression K004-05 avant K004-06" >&2
  exit 1
}

if grep -Eq '^\[(fpu|fpu-integrity)\] FAILED' <(tr -d '\r' <"$LOG"); then
  echo "[FAIL] K004-06: échec FPU ou corruption SIMD détectée par le noyau" >&2
  grep -E '\[(fpu|fpu-integrity)\] FAILED' "$LOG" >&2
  exit 1
fi

setup="$(tr -d '\r' <"$LOG" | grep -E '^\[fpu\] fxsr=' || true)"
summary="$(tr -d '\r' <"$LOG" | grep -E '^\[fpu-integrity\] tasks=' || true)"
if [[ -z "$setup" || -z "$summary" ]]; then
  echo "[FAIL] K004-06: lignes [fpu] ou [fpu-integrity] absentes" >&2
  exit 1
fi
sfield() { sed -nE "s/.* $1=([0-9a-z]+)( .*|$)/\1/p" <<<"$setup"; }
field() { sed -nE "s/.* $1=([0-9a-z]+)( .*|$)/\1/p" <<<"$summary"; }
triple() { sed -nE "s/.* $1=([0-9]+,[0-9]+,[0-9]+)( .*|$)/\1/p" <<<"$summary"; }

strategy="$(sfield strategy)"
if [[ "$MODE" == fxsave ]]; then
  if [[ "$strategy" != fxsave || "$(sfield forced_fxsave)" != 1 || "$(sfield area)" != 512 || "$(sfield align)" != 16 ]]; then
    echo "[FAIL] K004-06: repli FXSAVE forcé non appliqué: $setup" >&2
    exit 1
  fi
elif [[ "$(sfield xsave)" == 1 ]]; then
  if [[ "$strategy" != xsave || "$(sfield align)" != 64 ]]; then
    echo "[FAIL] K004-06: XSAVE disponible mais non retenu: $setup" >&2
    exit 1
  fi
  # XCR0 ne doit jamais dépasser x87|SSE|AVX.
  if (( ($(sfield xcr0) & ~0x7) != 0 || ($(sfield xcr0) & 0x3) != 0x3 )); then
    echo "[FAIL] K004-06: XCR0 hors de x87|SSE|AVX: $setup" >&2
    exit 1
  fi
fi

avx="$(field avx)"
if [[ "$(field strategy)" != "$strategy" || "$avx" != "$(sfield avx_enabled)" ]]; then
  echo "[FAIL] K004-06: stratégie ou AVX incohérents entre [fpu] et [fpu-integrity]" >&2
  exit 1
fi
expected_ymm=0
[[ "$avx" == 1 ]] && expected_ymm=16
if [[ "$(field xmm)" != 16 || "$(field ymm_high)" != "$expected_ymm" || "$(field mxcsr)" != 1 || "$(field corruptions)" != 0 ]]; then
  echo "[FAIL] K004-06: couverture SIMD ou corruption: $summary" >&2
  exit 1
fi
if (( $(field preemptions) < TARGET || $(field min_slices) < 10 )); then
  echo "[FAIL] K004-06: préemptions ou part minimale insuffisantes: $summary" >&2
  exit 1
fi

IFS=, read -r -a tasks <<<"$(triple tasks)"
IFS=, read -r -a busy <<<"$(triple busy_preemptions)"
IFS=, read -r -a iterations <<<"$(triple iterations)"
if [[ "${#tasks[@]}" -ne 3 ]] || [[ "$(printf '%s\n' "${tasks[@]}" | sort -u | wc -l)" -ne 3 ]]; then
  echo "[FAIL] K004-06: 3 tâches distinctes attendues: $summary" >&2
  exit 1
fi
for i in 0 1 2; do
  if (( ${busy[i]:-0} < MIN_BUSY_PREEMPTIONS )); then
    echo "[FAIL] K004-06: tâche ${tasks[i]} préemptée ${busy[i]:-0} fois dans sa boucle, au moins $MIN_BUSY_PREEMPTIONS attendues" >&2
    exit 1
  fi
  if (( ${iterations[i]:-0} == 0 )); then
    echo "[FAIL] K004-06: tâche ${tasks[i]} sans itération de vérification" >&2
    exit 1
  fi
done

if [[ "$(field exits)" != 3 || "$(field running_max)" != 1 || "$(field eoi_failures)" != 0 || "$(field canaries_ok)" != 3 ]]; then
  echo "[FAIL] K004-06: sorties, invariant RUNNING, EOI ou canaris invalides: $summary" >&2
  exit 1
fi

grep -Fq "ELYON_SCHED_FPU_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SCHED_FPU_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.4 non atteint après K004-06 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' "$LOG"; then
  echo "[FAIL] erreur pendant la qualification FPU" >&2
  exit 1
fi

echo "[PASS] K004-06 isolation FPU par tâche ($strategy, avx=$avx): xmm=16 ymm_high=$expected_ymm mxcsr intacts sous préemption, ${busy[0]},${busy[1]},${busy[2]} préemptions en boucle, aucune corruption"
