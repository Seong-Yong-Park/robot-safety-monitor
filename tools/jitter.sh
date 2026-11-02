#!/usr/bin/env bash
# 지터 측정 하네스 — 부하 없이 / CPU 부하를 걸고 `rsm run`을 여러 번 돌려
# 그룹 하나의 시작 지연(late) 분포를 표로 낸다. PRD §7 "지터 P99 ≤ 주기의 10%" 판정용.
#
#   tools/jitter.sh [-c CONFIG] [-g GROUP] [-n RUNS] [-d MILLIS] [-l LOAD_PROCS]
#
# 기본: demo-a, fast 그룹, 각 3회, 5초, 부하 프로세스 = CPU 코어 수.
# 부하는 바쁜 루프 셸 프로세스다. stress-ng 같은 도구를 따로 설치하지 않는다.
#
# Linux 전용. WSL2 에서 잰 값은 참고용이다 — 공식 수치는 전용 Linux + SCHED_FIFO
# 에서 다시 잰다 (PRD G4, TECH_STACK T1).
set -euo pipefail

cd "$(dirname "$0")/.."

config=examples/demo-a.yaml
group=fast
runs=3
duration=5000
load=$(nproc)

while getopts "c:g:n:d:l:h" opt; do
  case $opt in
    c) config=$OPTARG ;;
    g) group=$OPTARG ;;
    n) runs=$OPTARG ;;
    d) duration=$OPTARG ;;
    l) load=$OPTARG ;;
    *) sed -n '2,11p' "$0"; exit 2 ;;
  esac
done

cargo build --release -q -p rsm-cli
bin=target/release/rsm

# 설정상의 주기(µs). 판정 기준은 실효 주파수가 아니라 설정한 주기다.
nominal_hz=$("$bin" check --config "$config" | awk -v g="[$group]" '$1 == g { print $2 }')
if [[ -z $nominal_hz ]]; then
  echo "group '$group' not found in $config" >&2
  exit 1
fi

hogs=()
stop_load() {
  for pid in "${hogs[@]}"; do kill "$pid" 2>/dev/null || true; done
  hogs=()
}
trap stop_load EXIT

start_load() {
  for _ in $(seq "$load"); do
    sh -c 'while :; do :; done' &
    hogs+=("$!")
  done
}

echo "config $config · group $group (${nominal_hz} Hz) · ${runs} runs × ${duration} ms · load procs $load · $(nproc) CPUs"
echo
echo "| load | run | Hz | miss / tick | late p50 | p99 | p99.9 | max | mean | p99 / period |"
echo "|---|---|---|---|---|---|---|---|---|---|"

for mode in none cpu; do
  if [[ $mode == cpu ]]; then start_load; fi
  for r in $(seq "$runs"); do
    line=$("$bin" run --config "$config" --duration-ms "$duration" 2>&1 >/dev/null |
      grep "^\[rsm\] $group tick" || true)
    if [[ -z $line ]]; then
      echo "no summary line for group '$group'" >&2
      exit 1
    fi
    # [rsm] fast tick 1001 · 999.8 Hz · miss 0 · late p50 A p99 B p99.9 C max D mean E µs · ...
    echo "$line" | awk -v mode="$mode" -v run="$r" -v nominal="$nominal_hz" '{
      for (i = 1; i <= NF; i++) {
        if ($i == "tick")  ticks = $(i + 1)
        if ($i == "Hz")    hz = $(i - 1)
        if ($i == "miss")  miss = $(i + 1)
        if ($i == "p50")   p50 = $(i + 1)
        if ($i == "p99")   p99 = $(i + 1)
        if ($i == "p99.9") p999 = $(i + 1)
        if ($i == "max" && !seen_max) { max = $(i + 1); seen_max = 1 }
        if ($i == "mean" && !seen_mean) { mean = $(i + 1); seen_mean = 1 }
      }
      pct = 100 * p99 * nominal / 1e6
      printf "| %s | %d | %s | %s / %s | %s µs | %s µs | %s µs | %s µs | %s µs | %.1f%% |\n",
        mode, run, hz, miss, ticks, p50, p99, p999, max, mean, pct
    }'
  done
  if [[ $mode == cpu ]]; then stop_load; fi
done
