# 현재 상태

> 갱신: 2026-09-27 · 매 작업 세션 시작 시 이 문서를 먼저 본다.

## 한 줄

**Phase 0 구현 완료.** 설정 파일 하나로 파이프라인이 조립되어 돌고, 같은 바이너리가
`demo-a.yaml`(팔)과 `demo-b.yaml`(이동로봇)을 각각 실행한다. 다음은 Phase 1
(할당 0 검증, 지터 측정, ROS 2 어댑터 준비).

## 무엇이 되어 있나

| 영역 | 상태 |
|---|---|
| 의사결정 | **완료** — PRD v0.3(D1~D24), TECH_STACK v1.0(T1~T16) |
| Outside-in | **PRD 초안** — `docs/PRD-OUTSIDE-IN.md` v0.1, 결정 D-OI-1~7, 열린 질문 5. 구현 미착수 |
| 빌드·규칙 인프라 | **완료** — 워크스페이스, 툴체인 고정, clippy·cargo-deny 정책 |
| CI 워크플로 | 파일 작성됨, `.github/workflows/`에 배치 필요 |
| 문서 | README·CONTRIBUTING·BRIEF·DECISIONS·이 문서 |
| git | 로컬 저장소 + remote 연결 완료. **첫 push 미수행** |
| **rsm-core** | **완료** — 11개 모듈 전부 구현, 단위 테스트 34 + doctest 1 |
| **rsm-modules** | **완료** — Source 3종 · Detector 4종, 단위 8 + 통합 3 |
| **rsm-cli** | **완료** — `rsm run` / `rsm check` / `rsm list`, JSONL 출력 |
| 검증 | `cargo clippy --workspace --all-targets -- -D warnings` 무경고, 테스트 46 통과 |

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
| `supervisor.rs` | `HealthRegistry`(원자변수), 그룹 스테일·오버플로·고장 판정 |
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
| 주기 그룹 2개 독립 동작 | demo-a의 `fast` 1 kHz + `normal` 100 Hz | **확인** — 1.5초에 1371 tick, overrun 0 |
| 모듈 고장 격리 | `panic_probe`를 demo-b에 배치 | **확인** — 해당 모듈만 `Faulted`, 나머지 계속 동작 |
| 감시 불가 명시 | `stalling` + `comm_timeout` | **확인** — `availability`가 `Degraded`, `comm_timeout` CRITICAL |
| tick 경로 할당 0 | 전역 할당자 교체 후 카운터 확인 | **Phase 1** — 설계상 할당 없음, 계측 미구현 |
| 시계 주입 | 가상 시계로 tick을 손으로 밟는 통합 시험 | **확인** — `rsm-modules/tests/pipeline.rs` |

### 실측 (release 빌드, demo-a `fast` 그룹 = 1 kHz 설정)

| 환경 | tick 작업 시간 | 실제 tick 간격 | 유효 주파수 |
|---|---|---|---|
| Linux (컨테이너) | 평균 375 ns · 최대 18 µs | 1.09 ms | 917 Hz |
| Windows 11 | 평균 1.6 µs · 최대 45 µs | **1.88 ms** | **533 Hz** |

**측정치일 뿐 WCET 보장이 아니다** (PRD G4 단서 참조).

Windows는 `thread::sleep` 분해능 때문에 매 tick 약 0.9~1.6 ms가 밀린다.
**Windows는 개발·CI 전용으로 확정**했으므로 이 수치는 결함이 아니다 (PRD D16).
Windows에서 개발할 때는 `fast` 그룹 주기를 200 Hz 정도로 낮춰 쓴다.

**단, 계측이 이것을 못 잡았다.** 두 환경 모두 `overrun 0`으로 나왔다.
현재 `overran` 판정은 `elapsed > period`, 즉 **tick 작업 시간**만 본다.
데드라인을 놓치는 것은 보지 않는다. Phase 1 첫 항목으로 고친다.

## 다음 (Phase 1 후보)

- [ ] **주기 준수 계측 수정** — `overran`을 작업 시간이 아니라 **실제 tick 간격**
      (직전 `started` 와의 차) 기준으로 판정. 지금은 주기를 절반밖에 못 지켜도 0이 나온다
- [ ] tick 경로 할당 0 계측 — 시험용 전역 할당자로 카운트
- [ ] 지터 측정 하네스 — P50/P99/최대, 부하를 준 상태에서
- [ ] `--duration-ms` 대신 Ctrl-C 처리 (시그널 핸들러 의존성 결정 필요)
- [ ] JSONL 리플레이 — 기록한 줄을 `VirtualClock`으로 되돌려 같은 결과가 나오는지
- [ ] 메트릭 파사드 (T7)
- [ ] `rsm-core` 공개 API 문서 정리 + mdBook (나중)

## 지금 막혀 있는 것

| 항목 | 조치 |
|---|---|
| `LICENSE`가 자리표시자 | `Invoke-WebRequest https://www.apache.org/licenses/LICENSE-2.0.txt -OutFile LICENSE` |
| CI 워크플로 미배치 | `.github/workflows/ci.yml`·`heavy.yml` 저장 후 커밋 |
| 첫 push 미수행 | `git push -u origin main` (원격 저장소는 비어 있어 충돌 없음) |
| `Cargo.toml`의 `repository` | `https://github.com/OWNER/...` 자리표시자 → 실제 주소로 |

## 마일스톤

| | 범위 | 기간 |
|---|---|---|
| **v0.5** | 코어 + 레퍼런스 모듈 3종 + JSONL 리플레이 → **이때 저장소 공개** | 약 6개월 |
| **v1.0** | + ML 래퍼, ROS 2 어댑터, Gazebo 검증, 문서 | 누적 11~14개월 |
