# 현재 상태

> 갱신: 2026-11-02 · 매 작업 세션 시작 시 이 문서를 먼저 본다.

## 한 줄

**Phase 1 진행 중.** Phase 0(설정 하나로 조립되는 파이프라인, `demo-a`·`demo-b`)이
끝났다. Phase 1에서 **주기 준수 계측 수정**, **데드라인 미스 → `Degraded`**,
**tick 경로 할당 0 검증**, **지터 측정 하네스**를 마쳤다. 지터 P99는 WSL2에서 주기의
62~66%로 목표(10%)와 거리가 멀다 — 다음은 스레드 우선순위와 전용 Linux 재측정.

## 무엇이 되어 있나

| 영역 | 상태 |
|---|---|
| 의사결정 | **완료** — PRD v0.3(D1~D25), TECH_STACK v1.0(T1~T16). 열린 질문 1(PRD §10 Q-1) |
| Outside-in | **PRD 초안** — `docs/PRD-OUTSIDE-IN.md` v0.1, 결정 D-OI-1~7, 열린 질문 5. 구현 미착수 |
| 빌드·규칙 인프라 | **완료** — 워크스페이스, 툴체인 고정, clippy·cargo-deny 정책 |
| CI 워크플로 | 파일 작성됨, `.github/workflows/`에 배치 필요 |
| 문서 | README·CONTRIBUTING·BRIEF·DECISIONS·이 문서 |
| git | 로컬 저장소 + remote 연결. push 는 사용자가 한다 |
| **rsm-core** | **완료** — 12개 모듈 전부 구현, 단위 테스트 58 + doctest 1 |
| **rsm-modules** | **완료** — Source 3종 · Detector 4종, 단위 8 + 통합 3 |
| **rsm-cli** | **완료** — `rsm run` / `rsm check` / `rsm list`, JSONL 출력 |
| **rsm-alloc-check** | **완료** — 시험 전용 계측 할당자. 계측기 자체 시험 5 + fast 경로 시험 1 + doctest 1 |
| 검증 | `cargo clippy --workspace --all-targets -- -D warnings` 무경고, 테스트 77 통과, fast tick 경로 할당 0 |

### rsm-core 모듈

| 파일 | 역할 |
|---|---|
| `time.rs` | `Instant`/`Duration`/`Clock`/`MonotonicClock`/`VirtualClock` |
| `event.rs` | `Severity`·`Category`·`HazardEvent`·`EvidenceSet`·`SafetyState`·`MonitorAvailability` |
| `signal.rs` | `Signal` enum, `SignalSlot`, `SignalBus` |
| `intern.rs` | `Interner`, `Names` — 이름 → `u16` |
| `traits.rs` | `Source`·`Detector`·`ArbiterPolicy`·`Sink`·`EventSink`·`ModuleCtx`·`Params`·`Health` |
| `registry.rs` | 이름 → 팩토리 |
| `config.rs` | YAML 로드·검증·조립 (`PipelineConfig` → `BuiltPipeline`) |
| `schedule.rs` | `GroupWorker`·`ArbiterWorker`·`QueueSink`·`Runtime`·`run()` |
| `arbiter.rs` | `MaxSeverity` 융합 정책 |
| `supervisor.rs` | `HealthRegistry`(원자변수), 그룹 스테일·오버플로·고장·데드라인 미스 판정, 주기 통계(미스·시작 지연·작업 시간) |
| `histogram.rs` | 고정 칸 지연 히스토그램(크레이트 내부). 시작 지연 P50/P99/P99.9 |
| `error.rs` | `ConfigError` |

### rsm-modules

| `type:` | 종류 | 역할 |
|---|---|---|
| `ramp` | Source | 톱니파 신호 |
| `constant` | Source | 고정값 신호 |
| `stalling` | Source | N tick 뒤 갱신 중단 (통신 두절 재현) |
| `threshold` | Detector | 한계 초과/미달 |
| `min_distance` | Detector | 거리 2단계 (warning / critical) |
| `comm_timeout` | Detector | 갱신 끊김 |
| `panic_probe` | Detector | 일부러 패닉 (격리 시연용) |

## Phase 0 완료 판정

완료 조건: **더미 파이프라인이 2개 주기 그룹으로 돌고, 설정 파일만 바꿔 구성이 달라진다.**

| 항목 | 확인 방법 | 결과 |
|---|---|---|
| 설정만으로 구성 변경 | `demo-a` / `demo-b`를 같은 바이너리로 실행 | **확인** — 코드 변경 0 |
| 주기 그룹 2개 독립 동작 | demo-a의 `fast` 1 kHz + `normal` 100 Hz | **확인** — 둘 다 돈다. 단 주기는 못 지켰다(아래 실측). Phase 1에서 고침 |
| 모듈 고장 격리 | `panic_probe`를 demo-b에 배치 | **확인** — 해당 모듈만 `Faulted`, 나머지 계속 동작 |
| 감시 불가 명시 | `stalling` + `comm_timeout` | **확인** — `availability`가 `Degraded`, `comm_timeout` CRITICAL |
| tick 경로 할당 0 | 전역 할당자 교체 후 카운터 확인 | **확인 (Phase 1)** — `rsm-alloc-check/tests/fast_path.rs`. 아래 참고 |
| 시계 주입 | 가상 시계로 tick을 손으로 밟는 통합 시험 | **확인** — `rsm-modules/tests/pipeline.rs` |

### tick 경로 할당 0 (R-08)

`cargo test -p rsm-alloc-check`. 레퍼런스 Source 4개·Detector 4개(`panic_probe` 제외)를
한 fast 그룹에 넣고 1200 tick을 밟는다. tick마다 `GroupWorker::tick` + `record_group_tick`
구간의 할당·재할당·해제를 세어 **셋 다 0**이어야 통과다. 워밍업 없이 첫 tick부터 잰다.

- 측정 구간이 이벤트 냄/안 냄, `min_distance` 경고·치명, 큐 오버플로, 입력 멈춤 →
  `comm_timeout`을 모두 지나는지도 확인한다. 안 지나면 시험이 실패한다(헛통과 방지).
- 시험이 실제로 잡는지 확인했다: `QueueSink::emit`에 `Box` 하나를 일부러 넣자
  1200 tick 전부에서 할당 2·해제 2로 실패했다.
- **범위 밖**: 패닉 경로(`catch_unwind`가 패닉 내용을 `Box`에 담는다)와 Arbiter·Sink
  경로(JSON 직렬화).
- 가상 시각으로 밟으므로 실제 `group_loop`의 `thread::sleep`은 포함하지 않는다.

### 실측 (release 빌드, demo-a `fast` 그룹 = 1 kHz 설정)

**Phase 0 (수정 전)**

| 환경 | tick 작업 시간 | 실제 tick 간격 | 유효 주파수 |
|---|---|---|---|
| Linux (컨테이너) | 평균 375 ns · 최대 18 µs | 1.09 ms | 917 Hz |
| WSL2 Ubuntu 24.04 | 평균 1.7 µs · 최대 48 µs | 1.25 ms | 801 Hz |
| Windows 11 | 평균 1.6 µs · 최대 45 µs | **1.88 ms** | **533 Hz** |

세 환경 모두 `overrun 0`이었다. 버그가 둘이었다.

1. **스케줄러** — "예정 시각을 지났으면 예정 시각 = 지금"으로 위상을 되맞췄다.
   `thread::sleep`은 항상 조금 늦게 깨므로 이 조건이 매 tick 발동했고, 늦잠만큼
   주기가 늘었다. 그래서 늦잠이 큰 환경일수록 주파수가 낮았다.
2. **계측** — `overran = elapsed > period`, 즉 작업 시간만 봤다. 작업은 수 µs라
   주기를 절반밖에 못 지켜도 0이 나왔다.

**Phase 1 (수정 후, 2026-11-02, WSL2 Ubuntu 24.04, 2초 × 3회)**

| 실효 주파수 | 데드라인 미스 | 시작 지연 최대 · 평균 | 작업 시간 최대 · 평균 |
|---|---|---|---|
| 999.7 ~ 999.8 Hz | 0 · 1 · 0 / 2001 | 0.81 ~ 1.05 ms · 245 ~ 308 µs | 61 ~ 94 µs · 1.9 ~ 2.6 µs |

demo-b(100 Hz)는 99.9 Hz, 미스 0. 주파수는 이제 맞는다. 놓친 칸은 건너뛰므로
OS가 오래 안 깨워도 그 뒤 tick이 몰리지 않는다.

**남은 문제: 시작 지연이 크다.** 평균 250~300 µs는 주기의 25~30%로, PRD §7의
지터 목표(P99가 주기의 10% 이내)를 못 맞춘다. 2회차의 미스 1건은 OS가 1 ms 넘게
스레드를 안 깨운 것이다. WSL2는 실시간 스케줄링이 부정확하다(TECH_STACK T1).
공식 수치는 전용 Linux + `SCHED_FIFO`에서 다시 잰다. 그래도 크면 `timerfd`나
마지막 수십 µs만 바쁜 대기(spin)하는 방식을 검토한다(TECH_STACK T5).

### 지터 분포 (2026-11-02, `tools/jitter.sh`)

WSL2 Ubuntu 24.04(커널 6.18, 4 CPU), release 빌드, demo-a `fast`(1 kHz), 5초 × 3회.
부하는 CPU 수만큼 바쁜 루프 프로세스(같은 nice, 우선순위 설정 없음). 값은 칸 상한이라
실제보다 최대 6.25% 크다.

| 부하 | Hz | 미스 / tick | P50 | P99 | P99.9 | 최대 | P99 / 주기 |
|---|---|---|---|---|---|---|---|
| 없음 | 999.9 | 1 · 3 · 1 / 5001 | 221 ~ 262 µs | 622 ~ 655 µs | 885 ~ 918 µs | 1.13 ~ 1.26 ms | **62 ~ 66%** |
| CPU 4 | 904 ~ 956 | 215 ~ 297 / 4523 ~ 4781 | 70 ~ 74 µs | 3.0 ~ 5.0 ms | 10.0 ~ 11.5 ms | 11.2 ~ 17.5 ms | **302 ~ 498%** |

- **부하 없음도 목표(10%)를 못 맞춘다.** P50부터 주기의 22~26%다. 꼬리 문제가 아니라
  `thread::sleep`이 깨어나는 데 걸리는 기본 지연이다.
- **부하를 걸면 P50은 오히려 줄고 꼬리가 터진다.** 추정: vCPU가 계속 바빠 유휴 상태에서
  깨어나는 비용이 없어지지만(P50 감소), 감시 스레드가 바쁜 프로세스 4개와 같은 우선순위로
  CPU를 나눠 쓰므로 스케줄러 시간 조각(수 ms)만큼 밀린다(꼬리 증가). 미스 4.5~6.6%,
  실효 주파수 904~956 Hz.
- 두 번째가 T5에서 이미 채택한 `thread-priority`(SCHED_FIFO)가 겨냥하는 문제다. 아직 코드에
  적용하지 않았다. 첫 번째는 `timerfd`나 마지막 수십 µs 바쁜 대기(T5)를 검토할 근거다.
  둘 다 판정은 전용 Linux에서 한다(WSL2는 실시간 스케줄링이 부정확, T1).

**측정치일 뿐 WCET 보장이 아니다** (PRD G4 단서 참조).

Windows는 `thread::sleep` 분해능이 약 1 ms라 시작 지연이 그만큼 생긴다.
**Windows는 개발·CI 전용으로 확정**했으므로 이 수치는 결함이 아니다 (PRD D16).
Windows에서 개발할 때는 `fast` 그룹 주기를 200 Hz 정도로 낮춰 쓴다.

## 다음 (Phase 1 후보)

- [x] **주기 준수 계측 수정** — 데드라인 미스·시작 지연·실효 주파수로 나눠 기록.
      스케줄러의 위상 밀림도 같이 고침 (2026-11-02)
- [x] 데드라인 미스를 `availability`에 반영 — scan 구간에 새 미스가 있으면
      `Degraded`, 깨끗한 구간 하나면 회복 (PRD D25, 2026-11-02)
- [ ] 큐 오버플로 `Degraded`의 회복 규칙 — 지금은 sticky. PRD §10 Q-1
- [x] tick 경로 할당 0 계측 — `rsm-alloc-check` (2026-11-02)
- [ ] Miri — `unsafe`가 처음 생겼으므로 TECH_STACK T9의 도입 시점이다. nightly 툴체인 필요
- [x] 지터 측정 하네스 — 고정 칸 히스토그램 + `tools/jitter.sh` (2026-11-02)
- [ ] 그룹 스레드 우선순위 적용 — T5의 `thread-priority`(SCHED_FIFO, 권한 필요, 설정으로 선택).
      부하 시 꼬리 지연(P99 3~5 ms)이 겨냥하는 대상. 새 의존성이라 착수 전에 확인
- [ ] 전용 Linux에서 지터 재측정 — G4 공식 수치. 그래도 크면 `timerfd`/spin 검토
- [ ] `--duration-ms` 대신 Ctrl-C 처리 (시그널 핸들러 의존성 결정 필요)
- [ ] JSONL 리플레이 — 기록한 줄을 `VirtualClock`으로 되돌려 같은 결과가 나오는지
- [ ] 메트릭 파사드 (T7)
- [ ] `rsm-core` 공개 API 문서 정리 + mdBook (나중)

## 지금 막혀 있는 것

| 항목 | 조치 |
|---|---|
| `LICENSE`가 자리표시자 | `Invoke-WebRequest https://www.apache.org/licenses/LICENSE-2.0.txt -OutFile LICENSE` |
| CI 워크플로 미배치 | `.github/workflows/ci.yml`·`heavy.yml` 저장 후 커밋 |
| `Cargo.toml`의 `repository` | `https://github.com/OWNER/...` 자리표시자 → 실제 주소로 |

## 마일스톤

| | 범위 |
|---|---|
| **v0.5** | 코어 + 레퍼런스 모듈 3종 + JSONL 리플레이 → **이때 저장소 공개** |
| **v1.0** | + ML 래퍼, ROS 2 어댑터, Gazebo 검증, 문서 |
