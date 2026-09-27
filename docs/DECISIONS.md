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
| T3 | 설정 포맷 | YAML + `serde_yaml_ng` + `schemars`. `deny_unknown_fields`. `serde_yml` 금지 |
| T4 | 레지스트리 | 명시적 등록 함수. 팩토리 `fn(&Params) -> Result<Box<dyn Detector>>` 고정 |
| T5 | 스레드·큐 | `std::thread` + `thread-priority` + `rtrb`(전 구간 SPSC). **tokio 미사용** |
| T6 | 시간 | 자체 `Instant(u64 ns)` newtype + `trait Clock`. `MonotonicClock` / `VirtualClock` / (Phase 3) `RosClock` |
| T7 | 로깅·계측 | 3분리 — Sink `serde_json` / 진단 `tracing` / 계측 핫패스 고정 슬롯 → `metrics` |
| T8 | 에러·패닉 | `thiserror` + `anyhow` + `panic = "unwind"` + 모듈 tick `catch_unwind` + 패닉 모듈 **영구 `Faulted`** (재시작 정책은 P1 R-25. 2026-09-27 코드에 맞춰 변경) |
| T9 | 테스트 | `proptest`·`insta`·`loom`·Miri·`criterion`·`llvm-cov`·할당 카운터 (단계별 도입) |
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
| 큐 오버플로 정책 | **D23** — 들어오는 이벤트를 버리고 `Degraded` |
| Supervisor 자체 생존 | **D24** — Sink 주기 출력이 heartbeat. Supervisor 스레드만 멈추는 경우는 못 잡음 |
| 실행 파일 | **PRD §5.3** — `rsm-cli` (`run` / `check` / `list`) |

## Phase 0 실측에서 나온 결정

| 항목 | 결정 |
|---|---|
| Windows의 위치 | **개발·CI 전용.** 주기 성능(G4/R-03)의 판정 대상이 아니다. 실측에서 매 tick 1~1.6 ms가 밀려 1 kHz 설정이 533 Hz로 돌았다. `timeBeginPeriod(1)`은 `unsafe`라 코어에 들이지 않는다 (PRD D16 / D16-b) |
| 주기 준수 계측 | 현재 `overrun`은 **tick 작업 시간**만 본다. 데드라인을 놓쳐도 0으로 나온다. Phase 1에서 **실제 tick 간격**(직전 tick과의 차) 기준으로 바꾼다 |

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
