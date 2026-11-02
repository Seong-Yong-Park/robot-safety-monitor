# 결정 요약

결론만 모은 표다. **근거와 대안 비교는 [PRD.md](PRD.md) §9 와
[TECH_STACK.md](TECH_STACK.md) 각 절에 있다.**

결정을 뒤집을 때는 이 문서만 고치지 말고 근거 문서를 먼저 고친 뒤 여기를 맞춘다.

## 제품 결정 (PRD §9.1)

| ID | 결정 |
|---|---|
| D1 | 미들웨어 독립 코어 + 얇은 ROS 2 어댑터 |
| D2 | 코어 언어 **Rust**. Python은 도구(`rsm-tools`)에 한정 |
| D3 | 정적 레지스트리 + YAML 조합. 동적 플러그인은 P2 |
| D4 | Arbiter 기본 정책 **max-severity + TTL**. confidence 반영은 P1 |
| D5 | 시뮬레이터 1종 + 리플레이 병행 |
| D6 | v1은 **비인증 자문(advisory) 계층**. 안전 정지의 최종 책임은 로봇의 기존 안전 회로 |
| D7 | 알림 채널 — JSON Lines + stdout + (Phase 3) ROS 토픽 |
| D8 | *(운영 방침 — 비공개 문서에서 관리)* |
| D9 | ML은 **사전학습 모델 래퍼 1개**만 v1 레퍼런스에 포함. 모델 개발은 비목표 |
| D10 | 스케줄러는 **주기 그룹** — `fast` 1 kHz / `normal` 100 Hz / `async` 입력 주도 |
| D11 | **시계 외부 주입** (`Clock` 트레이트) |
| D12 | ROS 2 어댑터는 rclrs로 직접 구현 |
| D13 | 시뮬레이터 **Gazebo Harmonic**. 검증 순서 (b) 이동로봇 → (a) 매니퓰레이터. (c) 이족보행은 P1 이후 |
| D14 | ML은 **ONNX + `ort`**. v1은 CPU EP, CUDA는 feature. 모델 선정은 Phase 2 (라이선스 확인 필수) |
| D15 | 리플레이는 **코어 JSONL**, MCAP은 `rsm-mcap` Source로 격리 |
| D16 | 타겟 x86_64(Linux/Windows) + **aarch64 Linux**. CI 상시 빌드, 실기 검증 Phase 2 1회. 베어메탈 MCU는 대상 아님 |
| D17 | **오픈소스 공개, Apache-2.0 단독, DCO**. 공개 시점 v0.5 |
| D18 | 코딩 규칙 확정 — 아래 T10 |
| D19 | *(일정·공수 추정 — 비공개 문서에서 관리)* |
| D20 | 위험 타입 이름은 **`<category>.<subject>.<condition>`**. 첫 토막은 `Category`와 일치, `condition`은 위반을 말함, **같은 이름 = Arbiter가 합치는 단위**. 모듈 `name`은 인스턴스("어디서")이며 위험 이름과 독립 |
| D21 | **Outside-in을 같은 저장소에서 개발**, 권고 액션 필드(R-18)를 **P0로 승격**. 문서 경계: 코어 타입·계약을 바꾸면 `PRD.md`, 아니면 `PRD-OUTSIDE-IN.md` |
| D22 | 신호 타입은 **고정 enum** (`Bool`/`Scalar`/`Distance`/`Vector`). 확장은 `Custom` variant (예약·미구현) |
| D23 | 큐 오버플로는 **들어오는 이벤트를 버리고 `Degraded`**. 블로킹하지 않는다. `Unavailable`이 아니다 — 그룹은 돌고 있다 |
| D24 | Supervisor 생존은 **Sink 주기 출력(`on_change_only: false`)을 heartbeat로.** 외부 watchdog은 R-21 |
| D25 | 데드라인 미스는 **scan 구간 판정으로 `Degraded`.** 직전 scan 이후 새 미스가 있으면 `Degraded`, 깨끗한 구간 하나면 회복. `Unavailable`이 아니다. 오버플로도 같은 규칙(D26) |
| D26 | 큐 오버플로도 **scan 구간 판정으로 회복**(Q-1 해소). 누적 횟수는 통계에 남고, `Degraded` 구간은 JSONL에 남는다(그래서 Arbiter Hz ≥ Supervisor Hz를 설정 검증으로 강제). 모듈 `Faulted`는 sticky 유지 |
| D27 | 권고 액션(R-18) = **고정 enum + Arbiter 심각도→권고 매핑(설정) + 감시 불가 시 권고 설정 필수**. 절이 없으면 기능 꺼짐. 값 = `none` / `slow_down` / `stop`(의도만 정의, 크기·방식은 제어기), 심각도가 오를수록 약해지는 매핑은 거부 |
| D28 | JSONL 리플레이 v0.5 범위 = **재생만**(리플레이 Source + `rsm replay` + `insta` 스냅샷). 기록 기능은 그 뒤. **구현됨** (2026-11-02) |

## Outside-In 결정 (`PRD-OUTSIDE-IN.md` §7)

| ID | 결정 |
|---|---|
| D-OI-1 | Halos 구조를 **참조하되 독자 구현**. x86-64 + GPU, NVIDIA 하드웨어 비종속 |
| D-OI-2 | **권고만** 낸다 (R-18). 명령은 발행하지 않는다 |
| D-OI-3 | 같은 저장소에 `rsm-vision` · `rsm-command` 크레이트 추가 |
| D-OI-4 | **인지는 별도 프로세스.** `rsm-vision`은 인지 결과 인터페이스 + 참조 구현. 현장별 인지는 교체 단위 |
| D-OI-5 | 검증은 **인지 결과 JSONL 리플레이(CI) + 실기 카메라 녹화**. 시뮬레이터 미도입 |
| D-OI-6 | 첫 검증 현장은 **실내 연구실의 휴머노이드(Unitree G1)** |
| D-OI-7 | 코드·인터페이스·참조 구현은 저장소에, **현장별 모델 가중치·구역 설정은 저장소 밖에** (D14 확장) |

## 기술 스택 결정 (TECH_STACK v1.0)

| ID | 요소 | 결정 |
|---|---|---|
| T1 | 개발 환경 | Phase 0~2 Windows 네이티브. Linux 환경은 Phase 3에 결정 (Ubuntu 24.04 기준) |
| T2 | 툴체인 | stable + edition 2024, `rust-toolchain.toml`로 1.98.1 고정 |
| T3 | 설정 포맷 | YAML + `serde_yaml_ng` + `schemars`. `deny_unknown_fields`. `serde_yml` 금지. `unsafe-libyaml` 관리 중단 권고는 예외 처리, 공개 전 교체 재검토 |
| T4 | 레지스트리 | 명시적 등록 함수. 팩토리 `fn(&Params) -> Result<Box<dyn Detector>>` 고정 |
| T5 | 스레드·큐 | `std::thread` + `thread-priority` + `rtrb`(전 구간 SPSC). **tokio 미사용**. 놓친 주기는 건너뛰고 위상 유지. 그룹별 `rt_priority`(SCHED_FIFO, Linux 전용) — 적용 못 하면 기동 실패. `timerfd`는 하지 않고, spin은 전용 Linux 측정 뒤 결정 |
| T6 | 시간 | 자체 `Instant(u64 ns)` newtype + `trait Clock`. `MonotonicClock` / `VirtualClock` / (Phase 3) `RosClock` |
| T7 | 로깅·계측 | 3분리 — Sink `serde_json` / 진단 `tracing` / 계측 핫패스 고정 슬롯 → `metrics`. 지연 분포는 고정 칸 히스토그램(로그-선형, 오차 ≤ 6.25%, 칸 상한으로 보고) |
| T8 | 에러·패닉 | `thiserror` + `anyhow` + `panic = "unwind"` + 모듈 tick `catch_unwind` + 패닉 모듈 **영구 `Faulted`** (재시작 정책은 P1 R-25. 2026-09-27 코드에 맞춰 변경) |
| T9 | 테스트 | `proptest`·`insta`·`loom`·Miri·`criterion`·`llvm-cov`·할당 카운터 (단계별 도입). 할당 카운터는 시험 전용 `rsm-alloc-check`에 격리 |
| T10 | 코딩 규칙 | core `forbid(unsafe_code)` / `clippy::pedantic` / `unwrap`·`expect`·`panic` deny / `indexing_slicing`은 핫패스 한정 / `cargo-deny` |
| T11 | ML 런타임 | ONNX + `ort`. CPU 시작, CUDA feature. `ort-tract` 대체 여지 |
| T12 | ROS 2 | **Jazzy Jalisco** (EOL 2029-05, Ubuntu 24.04). rclrs 수동 설치 |
| T13 | 시뮬레이터 | Gazebo Harmonic |
| T14 | 리플레이 포맷 | JSONL(대용량은 파일 참조) + `rsm-mcap` 격리 |
| T15 | CI | GitHub Actions 5잡. aarch64 상시(`rsm-ml` 제외), `cross` |
| T16 | 문서·구조 | rustdoc + doctest. 가이드는 `docs/*.md`로 시작, mdBook은 Phase 2. 단일 리포 |

## Phase 0에서 확정한 설계 세부 (옛 PRD §10 열린 질문)

**전부 PRD로 옮겼다.** 이 표는 한때 PRD를 거치지 않고 결정을 직접 담아 두 번째
정본이 됐고, 그 사이 PRD §10에는 같은 항목이 "미정"으로 남아 있었다. 이제는 가리키기만 한다.

| 항목 | 어디로 |
|---|---|
| 위험 타입 네이밍 | **D20** — 3단 계층 문자열 |
| 신호 타입 시스템 | **D22** — 고정 enum. `Custom`은 예약·미구현 |
| 큐 오버플로 정책 | **D23** — 들어오는 이벤트를 버리고 `Degraded`. **D26** — 새 오버플로가 없는 scan 구간이면 회복 |
| Supervisor 자체 생존 | **D24** — Sink 주기 출력이 heartbeat. Supervisor 스레드만 멈추는 경우는 못 잡음 |
| 데드라인 미스 | **D25** — scan 구간에 새 미스가 있으면 `Degraded`, 없으면 회복 |
| 실행 파일 | **PRD §5.3** — `rsm-cli` (`run` / `check` / `list`) |

## Phase 0 실측에서 나온 결정

| 항목 | 결정 |
|---|---|
| Windows의 위치 | **개발·CI 전용.** 주기 성능(G4/R-03)의 판정 대상이 아니다. 실측에서 매 tick 1~1.6 ms가 밀려 1 kHz 설정이 533 Hz로 돌았다. `timeBeginPeriod(1)`은 `unsafe`라 코어에 들이지 않는다 (PRD D16 / D16-b) |
| 주기 준수 계측 | **반영됨(Phase 1).** `overrun`(작업 시간 > 주기)을 없애고 셋으로 나눴다 — **데드라인 미스**(`ended > release + period`), **시작 지연**(`started − release`, 최대·평균, 그리고 고정 칸 히스토그램으로 P50·P99·P99.9), **작업 시간**. 여기에 첫·마지막 tick 간격으로 잰 **실효 주파수**를 더했다. 원래 계획한 "직전 tick과의 간격"은 쓰지 않았다 — 위상이 밀리는 버그가 있으면 간격이 고르게 길어질 뿐이라 예정 시각(release)과 비교해야 보인다 |
| 주기 스케줄러 | 실측 중 계측만이 아니라 **스케줄러 자체**도 틀렸음이 드러났다. "예정 시각을 지났으면 예정 시각 = 지금"이 `sleep` 늦잠 때문에 매 tick 발동해 주기가 늘어났다. 놓친 주기는 건너뛰고 위상은 유지하도록 바꿨다 (TECH_STACK T5). Arbiter·Supervisor 루프도 같은 함수를 쓴다 |

### 여기서 파생된 제약 하나

R-08이 `fast` 그룹 tick 경로의 힙 할당 0을 요구하므로, `HazardEvent`에
`String`·`Vec`을 넣을 수 없다. 모듈 이름과 위험 타입은 **설정 로드 시점에 인터닝한
정수 ID**로 다루고, 근거(evidence)는 고정 크기 배열에 담는다. 문자열로 되돌리는 것은
Sink에서 출력할 때뿐이다.

## 후속 확인 항목

| 항목 | 시점 |
|---|---|
| ML 모델 선정 + 라이선스 확인 (YOLO 계열 상당수가 AGPL-3.0) | Phase 2 착수 |
| rclrs의 Jazzy 지원·기능 커버리지 재확인, 설치 절차 스크립트화 | Phase 3 착수 |
| ARM 실기 검증 1회 | Phase 2 |
| DCO → CLA 전환 재검토 | 외부 기여 발생 시 |
| `hazard` 접두어 ↔ `Category` 불일치를 `configure` 기동 실패로 강제, 데모 모듈 `name` 정리 (D20-a) | Phase 1 |
