# 기술 스택 결정서 — Robot Safety Monitor (RSM)

| 항목 | 내용 |
|---|---|
| 문서 버전 | v1.0 (리서치 기준일 2026-09-12, 결정 완료 2026-09-15) |
| 상태 | **T1~T16 전 항목 확정.** 각 절의 "결정"이 유효한 내용이며, "추천"은 결정 당시의 초안 근거로 보존 |
| 전제 (PRD v0.2) | Rust 코어 · 미들웨어 독립 코어 + rclrs 어댑터 · 주기 그룹 스케줄러 · 시계 주입 · 정적 레지스트리 + YAML · ML 래퍼 1개 · 비인증 자문 계층 |
| 범위 | 기술 조합 결정. 실제 구축 아님 |

각 항목은 **개념 → 후보 비교 → 추천 → 결정** 순서로 적는다. "결정" 칸은 검토 후 채운다.

---

## T1. 개발 환경 (호스트 OS · 빌드 환경)

**개념.** Rust 코어 자체는 Windows/Linux/macOS 어디서나 빌드되지만, ROS 2와 Gazebo는 사실상 Ubuntu가 Tier 1이다. 로컬 폴더가 `D:\`이므로 호스트는 Windows로 보이며, ROS 2 어댑터(Phase 3)부터는 Linux 환경이 필수다.

| 후보 | 특징 | 비고 |
|---|---|---|
| **WSL2 + Ubuntu 26.04** | Windows 유지, GPU(CUDA) 패스스루 가능, Gazebo GUI는 WSLg로 동작 | 실시간 스케줄링(`SCHED_FIFO`) 지터 측정은 부정확 — 지터 수치는 참고용 |
| 듀얼부트 / 전용 Linux 머신 | 지터 측정 신뢰도 최고, 실기 연결 용이 | 개발 편의성 저하 |
| Docker devcontainer (ROS 2 공식 이미지) | 환경 재현성, CI와 동일 이미지 | GUI·GPU 설정 번거로움. WSL2와 병행 가능 |

**추천.** Phase 0–2는 **WSL2 + Ubuntu 26.04** (코어는 미들웨어 독립이라 Phase 0–2는 Windows 네이티브 Rust로도 가능). Phase 3부터 devcontainer를 CI와 공유. 지터 측정은 나중에 전용 Linux에서 재측정한다는 단서를 PRD G4에 추가.

**결정.** **Phase 0–2는 Windows 네이티브 Rust. Linux 환경(WSL2/듀얼부트/devcontainer)은 Phase 3 착수 시 결정.** 단서: Windows에서 측정한 지터는 참고용이며 G4 공식 수치는 Linux에서 재측정. CI는 처음부터 Linux 매트릭스를 포함해 코어가 양쪽에서 빌드됨을 보장.

---

## T2. Rust 툴체인

**개념.** 채널(stable/beta/nightly), 에디션(2015/2018/2021/2024), MSRV(Minimum Supported Rust Version) 정책. 에디션은 문법·기본 규칙의 세대이고 크레이트마다 독립적으로 선택하며 상호 호환된다. 2024 에디션은 `unsafe` 관련 규칙 강화(`unsafe extern`, `unsafe` 속성 명시 등)가 포함되어 R-22 방향과 맞는다.

| 후보 | 특징 |
|---|---|
| **stable + edition 2024** | 안정성, 툴 지원 완전. 에디션 2024의 강화된 unsafe 규칙 |
| nightly | 일부 실험 기능(`#![feature]`). 사업화 제품에 부적합 |
| 인증 툴체인(Ferrocene) | ISO 26262/IEC 61508 인증된 rustc 배포판. 유료. v1(비인증 자문 계층)에는 불필요, 포지셔닝 상향 시 검토 |

**추천.** **stable, edition 2024, `rust-toolchain.toml`로 버전 고정**, MSRV = 고정 버전 - 2 마이너 정도로 문서화. 6개월마다 상향.

**결정.** **stable + edition 2024 + `rust-toolchain.toml` 버전 고정.** MSRV 문서화, 6개월 주기 상향.

---

## T3. 설정 파일 포맷 · 파서

**개념.** PRD R-02의 선언적 조합 파일. 사람이 손으로 쓰고, 기동 시 스키마 검증(R-02: 오류는 기동 실패)이 필요하다. Rust에서는 `serde`가 사실상 표준 직렬화 프레임워크이며, 포맷별 어댑터 크레이트를 고른다.

| 후보 | 특징 | 주의 |
|---|---|---|
| **YAML** | ROS 2 파라미터 파일·launch와 같은 포맷, 중첩 구조에 강함 | 원조 `serde_yaml`은 2024-03 **deprecated**. 유지보수 포크 `serde_yaml_ng`가 커뮤니티 권장. `serde_yml`은 **회피 권장**(커뮤니티 신뢰 문제·보안 권고 이력). YAML 특유의 암묵적 타입 변환(`no`→false, `1e3` 등) 주의 |
| TOML | Rust 생태계 표준(`toml` 크레이트, 안정적) | 깊은 중첩·리스트-오브-맵 표현이 장황. 파이프라인 그래프 표현에 불리 |
| JSON5 / RON | 주석 허용, 타입 명확 | ROS 생태계와 이질적 |

**스키마 검증.** `schemars`로 Rust 타입 → JSON Schema 생성 → `rsm-tools`(Python)에서 검증·에디터 자동완성에 재사용. 코어 내부 검증은 serde 역직렬화 + 수동 교차 검증(미연결 입력 등).

**추천.** **YAML (`serde_yaml_ng`) + `schemars`**. PRD에서 이미 YAML을 전제했고 ROS 2 파라미터 파일과 통일된다. `deny_unknown_fields`를 기본으로 켜서 오타를 기동 실패로 만든다.

**결정.** **YAML (`serde_yaml_ng`) + `schemars`.** `deny_unknown_fields` 기본, `serde_yml` 사용 금지(cargo-deny ban 목록에 추가).

---

## T4. 모듈 레지스트리 메커니즘

**개념.** R-03 "이름 → 팩토리" 등록. Rust에는 C++의 정적 초기화 순서 같은 것이 없어서(생성자 함수 없음), 등록 방식을 명시적으로 고른다.

| 후보 | 동작 | 장단점 |
|---|---|---|
| **명시적 등록 함수** | `rsm_modules::register_all(&mut registry)`가 모든 모듈을 코드로 나열 | 단순·결정적·`no_std` 친화. 모듈 추가 시 목록 1줄 수정(코어 아님, 모듈 크레이트) |
| `inventory` 크레이트 | `inventory::submit!`로 분산 등록, 링크 시 수집 | 목록 수정 불필요. 링커 섹션 트릭(`.init_array`) 사용 — 일부 타겟·정적 링크에서 문제 이력 |
| `linkme` 크레이트 | 분산 슬라이스, 링크 타임 수집 | `inventory`와 유사, 런타임 초기화 코드 없음. 역시 링커 의존 |
| 빌드 스크립트 코드 생성 | `build.rs`가 모듈 디렉터리 스캔 → 등록 코드 생성 | 유연하지만 빌드 복잡도 ↑ |

**추천.** **명시적 등록 함수**. G2 "코어 수정 0줄"은 만족하고(수정은 `rsm-modules`), P2 동적 로드 시 팩토리 시그니처 `fn(&Params) -> Result<Box<dyn Detector>>`를 그대로 `dlsym` 출처로 교체 가능. 링커 트릭은 임베디드 타겟(D16)에서 리스크.

**결정.** **명시적 등록 함수.** 팩토리 시그니처 `fn(&Params) -> Result<Box<dyn Detector>>` 고정(P2 `dlsym` 호환).

---

## T5. 스레드 · 스케줄링 · 그룹 간 큐

**개념.** PRD §5.2 주기 그룹. 그룹당 스레드 1개, 주기 타이머, 그룹 간 lock-free 큐. 비동기 런타임(tokio)은 **사용하지 않는다** — 작업 스틸링 스케줄러는 결정성과 상반되고, 실시간 스레드 우선순위를 다룰 수 없다.

| 요소 | 후보 | 추천 |
|---|---|---|
| 스레드 | `std::thread` + `thread-priority` 크레이트(SCHED_FIFO/RR, 코어 pinning은 `core_affinity`) | 채택. RT 우선순위는 설정으로 선택(권한 필요) |
| 주기 타이머 | `std::thread::sleep` 기반 절대 시각 대기(`sleep_until` 유사 구현) / Linux `timerfd`(nix) | v1은 절대 시각 sleep, 지터 계측 후 필요 시 timerfd |
| 그룹 간 큐 (1:1) | **`rtrb`** — 실시간 안전 SPSC 링버퍼(오디오 커뮤니티 검증, 할당 없음, wait-free) | 채택 |
| 그룹 간 큐 (N:1, 여러 그룹 → Arbiter) | `crossbeam::ArrayQueue`(bounded MPMC) / `ringbuf` | SPSC를 그룹 수만큼 두고 Arbiter가 폴링 → `rtrb`만으로 통일 가능 |
| 오버플로 정책 | drop-oldest vs `MonitorUnavailable` 승격 | 안전 관점 후자(PRD §10) |

**추천.** `std::thread` + `thread-priority` + `rtrb`(모든 그룹 간 통신을 SPSC로 정규화). 큐 용량은 설정에서 고정.

**결정.** **`std::thread` + `thread-priority` + `rtrb`.** 그룹당 OS 스레드 1개, 절대 시각 기준 `sleep_until` 루프(지터 부족 시 `timerfd`로 교체), 그룹마다 SPSC 큐 1개를 두고 Arbiter가 순회 폴링. tokio 미사용. 오버플로 정책은 PRD §10에서 Phase 1 중 확정.

---

## T6. 시간 (Clock 트레이트 구현체)

**개념.** PRD D11. 코어는 `trait Clock { fn now(&self) -> Instant; }`를 받는다. 구현체 선택 문제.

| 구현체 | 용도 |
|---|---|
| `std::time::Instant` 래퍼(단조 시계) | 실기 기본 |
| 가상 시계(`AtomicU64` 나노초 + 수동 전진) | 리플레이·단위 테스트 |
| ROS 시계(`/clock`, `use_sim_time`) 래퍼 | `rsm-ros2` |
| `quanta` 크레이트(TSC 기반 고해상도) | 지터 계측 정밀도가 부족할 때만 |

**추천.** 코어의 `Instant` 타입을 `std::time::Instant`가 아닌 **자체 `Instant(u64 ns)` newtype**으로 정의(ROS 시계·가상 시계와 호환, 직렬화 가능). 위 3종 구현체를 v1에 포함.

**결정.** **자체 `Instant(u64 ns)` newtype + `trait Clock { fn now(&self) -> Instant }`.** v1 구현체: `MonotonicClock`(실기, 내부에서만 `std::time::Instant` 사용), `VirtualClock`(리플레이·테스트, `AtomicU64` 수동 전진). `RosClock`은 Phase 3 `rsm-ros2`에. 벽시계(`SystemTime`)는 판정에 사용 금지(Sink에서 표시용 오프셋만). `rsm-core`에서 `std::time::Instant::now()`/`SystemTime::now()` 직접 호출은 clippy `disallowed_methods`로 차단. `quanta`는 계측 해상도 부족 시에만 추가.

---

## T7. 로깅 · 계측

**개념.** JSON Lines Sink(D7)와 내부 진단 로그·계측(tick 시간, 초과 횟수)의 도구.

| 후보 | 특징 |
|---|---|
| **`tracing` + `tracing-subscriber`(json)** | 구조화 로그 표준. span으로 tick 단위 계측 가능. 비동기·동기 모두 지원 |
| `log` + `env_logger` | 단순. 구조화 필드 지원 약함 |
| `metrics` 크레이트 | 카운터/히스토그램 표준 인터페이스. 지터 P99 산출에 적합 |

**주의.** `fast` 그룹 tick 경로에서는 로깅 호출이 할당·락을 유발하면 안 된다(R-08). 핫패스에서는 `tracing` 매크로를 쓰지 말고 고정 크기 계측 슬롯에 기록 → 별도 스레드가 배출.

**추천.** **`tracing`(진단) + `metrics`(계측)**, JSON Lines Sink는 `serde_json` 직접 사용(로깅 프레임워크와 분리 — Sink는 프레임워크의 출력이지 로그가 아님).

**결정.** **세 출력 분리.** (a) 이벤트 Sink = `serde_json` 직접(스키마 고정, 별도 스레드). (b) 진단 로그 = `tracing` + `tracing-subscriber`(텍스트/JSON 선택, `tracing-log` 브리지), 핫패스 밖에서만 호출. (c) 계측 = 핫패스는 그룹별 고정 슬롯(원자 카운터 + 고정 링버퍼, 할당·락 없음), 배출 스레드가 `metrics` 파사드로 이관; exporter는 v1 종료 시 요약 JSON 덤프, P1(R-14)에서 Prometheus. 1 kHz tick 루프 안에서 `tracing`/`metrics` 매크로 호출 금지(clippy `disallowed_macros`로 핫패스 모듈에 적용).

---

## T8. 에러 처리 · 패닉 격리

**개념.** 라이브러리는 타입화된 에러, 바이너리는 컨텍스트 체인. R-07 고장 격리는 패닉 경계가 필요하다.

| 요소 | 후보 | 추천 |
|---|---|---|
| 라이브러리 에러 | **`thiserror`**(enum 정의) / `snafu` | `thiserror` |
| 바이너리 에러 | **`anyhow`** / `eyre` | `anyhow` |
| 패닉 정책 | `panic = "unwind"` + `std::panic::catch_unwind` 경계 / `panic = "abort"` | **unwind + 모듈 tick마다 `catch_unwind`**. abort는 R-07과 양립 불가 |
| 패닉 후 상태 | `AssertUnwindSafe` + 모듈 재생성(팩토리 재호출) | 모듈 상태를 버리고 재생성 — 오염 상태 재사용 금지 |

**결정.** **`thiserror`(라이브러리) + `anyhow`(바이너리) + `panic = "unwind"` + 모듈 tick 단위 `catch_unwind` + 패닉 모듈 재생성.** 재시작 정책(즉시 / N회 후 영구 `Faulted`)은 설정 항목. fail-fast(즉시 프로세스 종료)는 외부 watchdog·프로세스 관리자가 전제되어야 하므로 v1 범위 밖 — 대신 PRD §10의 "Supervisor 자체 생존 탐지"를 Phase 1에서 함께 설계. `extern "C"` 경계(P2 R-19)에서는 `catch_unwind`가 선택이 아닌 필수(패닉이 C 스택을 통과하면 UB).

---

## T9. 테스트 도구

| 목적 | 후보 | 추천 |
|---|---|---|
| 단위/통합 | `cargo test` | 기본 |
| 속성 기반 | **`proptest`** / `quickcheck` | `proptest`(Arbiter 정책·TTL 불변식) |
| 스냅샷 (리플레이 기대 시퀀스) | **`insta`** | 이벤트 시퀀스를 스냅샷 파일로 관리, 회귀 diff |
| 동시성 모델 검사 | **`loom`** | 그룹 간 큐·Supervisor 상호작용의 인터리빙 탐색 |
| UB 검출 | **Miri** (`cargo miri`) | `unsafe` 포함 코드 경로 |
| 벤치마크 | **`criterion`** | tick 실행 시간 회귀 |
| 커버리지 | `cargo-llvm-cov` | CI 리포트 |
| 할당 검증 | 커스텀 `GlobalAlloc` 카운터 테스트 | R-08 "fast 경로 할당 0" |

**결정.** **추천안 전체 채택** — `cargo test` + `proptest` + `insta` + `loom` + Miri + `criterion` + `cargo-llvm-cov` + 커스텀 할당 카운터. 도입 시점은 필요에 맞춰 순차적으로(할당 카운터·`insta`는 Phase 1 필수, `proptest`는 Arbiter 구현 시, `loom`은 그룹↔Arbiter 큐 연동 시, Miri는 `unsafe` 최초 등장 시, `criterion`은 지터 계측 착수 시). CI에서 Miri·`loom`은 비용이 크므로 주기 실행(예: 주 1회 또는 해당 크레이트 변경 시)으로 분리.

---

## T10. 정적 분석 · 코딩 규칙 (PRD D18)

**개념.** 첫 커밋부터 CI에 걸 규칙 세트. R-22(무결성 상향 대비)의 실질적 담보.

| 규칙 | 제안 |
|---|---|
| `unsafe` | `rsm-core`: `#![forbid(unsafe_code)]`. FFI·큐 등 불가피한 곳은 별도 크레이트(`rsm-sys`)로 격리 + `// SAFETY:` 주석 필수 |
| lint | `clippy::all` + `clippy::pedantic`(선별 allow) + `clippy::unwrap_used`, `expect_used`, `panic` → deny (핫패스 크레이트) |
| 포맷 | `rustfmt` 기본 |
| 의존성 정책 | **`cargo-deny`**(라이선스 허용 목록, 중복·yanked·advisory) + `cargo-audit` |
| 공급망 | `Cargo.lock` 커밋, `cargo-vet` 또는 `cargo-crev`는 사업화 시점에 |
| unsafe 통계 | `cargo-geiger`로 의존성 unsafe 양 추적 |
| 문서 | `#![warn(missing_docs)]` (`rsm-core` 공개 API) |

**결정.** 추천안 채택 + 두 항목 조정. **`unsafe`**: `rsm-core`에 `#![forbid(unsafe_code)]`, 불가피한 것은 `rsm-sys`로 격리(`deny(unsafe_op_in_unsafe_fn)` + `// SAFETY:` 주석 강제). **패닉 lint**: `unwrap_used`/`expect_used`/`panic`을 라이브러리 크레이트에 `deny`(바이너리·테스트는 예외), **`indexing_slicing`은 `rsm-core` 핫패스 모듈에만 적용**. **`clippy::pedantic` 켜기**(`warn` 레벨, CI에서 `-D warnings`로 승격, 과한 항목만 개별 `allow`). **의존성**: `cargo-deny` 하나로 통합(licenses 허용 목록·GPL 차단, bans에 `serde_yml`, advisories, sources), `cargo-audit` 별도 도입 안 함. `cargo-geiger`는 참고 지표(게이트 아님), `cargo-vet`/`cargo-crev`는 사업화 단계로 보류. **포맷**: `rustfmt` 기본 + `cargo fmt --check`. CI 순서: fmt → clippy → deny → test → llvm-cov, Miri·loom은 주기 실행.

---

## T11. ML 추론 런타임 (PRD D14)

**개념.** `rsm-ml`의 사전학습 모델 래퍼. 모델 포맷은 **ONNX**로 고정(프레임워크 독립, YOLO 계열 공식 export 지원). 런타임 선택.

| 후보 | 특징 | 장단점 |
|---|---|---|
| **`ort`** (ONNX Runtime 바인딩) | 2.0.0-rc.13, ONNX Runtime v1.28 바인딩. CUDA/TensorRT/OpenVINO/QNN 등 실행 공급자 | 성능·커버리지 최고. C++ 라이브러리 링크(순수 Rust 아님). API 2.0이 rc 단계 |
| `tract` | 순수 Rust, CPU 추론 | 의존성 단순, 임베디드 친화. GPU 없음, 연산자 커버리지 제한 |
| `candle` | HuggingFace 순수 Rust, CUDA 지원 | ONNX 직접 로드 제한적(자체 포맷 위주). LLM 지향 |
| `ort` + `ort-tract` 백엔드 | `ort` API 그대로, 백엔드만 순수 Rust로 교체 | 코드 변경 없이 GPU↔CPU-only 전환 가능 |

**추천.** **`ort`를 API로 고정**, v1은 CPU 실행 공급자로 시작, GPU(CUDA)는 feature flag. 순수 Rust 필요 시 `ort-tract` 백엔드로 교체. 모델: YOLO 계열 ONNX(사람 클래스만 사용).

**결정.** **모델 포맷은 ONNX 고정, 런타임 API는 `ort`.** v1은 CPU 실행 공급자로 시작하고 CUDA는 Cargo feature로 분리(GPU 자원 점유 검증은 최소 1회 수행). 순수 Rust가 필요해지면 `ort-tract` 백엔드로 교체(코드 변경 없음). `rsm-ml`은 feature로 분리해 없이도 빌드 가능.

**모델 라이선스 주의(Phase 2 착수 시 확인).** 사전학습 모델에도 라이선스가 붙고, YOLO 계열 중 상당수(Ultralytics YOLOv5/v8/v11 등)는 **AGPL-3.0**이라 사업화 의도(D8)와 충돌한다. 허용적 라이선스 모델(예: YOLOX 계열 Apache-2.0)을 우선 검토하되, 라이선스는 버전·배포처마다 다르므로 **선정 시점에 해당 저장소 LICENSE를 직접 확인**한다. 모델 파일은 저장소에 동봉하지 않고 설정 파일에서 경로를 받는다(배포 부담·라이선스 전염 회피).

---

## T12. ROS 2 배포판 · rclrs 버전

**개념.** `rsm-ros2`가 대상으로 하는 배포판. rclrs는 **0.7.0 (2026-01-18)** 이 최신이며, Rolling·Lyrical에서는 메시지 패키지·코드 생성기가 사전 설치되어 별도 클론이 불필요하다.

| 후보 | 상태 | 비고 |
|---|---|---|
| **Lyrical Luth** | LTS, 2026-05-22 출시, EOL 2031-05, Ubuntu 26.04 Tier 1 (amd64/arm64) | rclrs 사전 통합, Gazebo Jetty 페어링, GPU zero-copy 버퍼(`rosidl::Buffer`) 신규 |
| Jazzy Jalisco | LTS, 2024-05, EOL 2029-05, Ubuntu 24.04 | 생태계 성숙, 서드파티 패키지 커버리지 넓음. rclrs는 수동 설치 |
| Kilted Kaiju | non-LTS, 2025-05, EOL 2026-11 | 곧 EOL. 제외 |

**추천.** **Lyrical Luth**. 개인 프로젝트라 레거시 호환 부담이 없고, rclrs 사전 통합·5년 지원·arm64 Tier 1(D16 Jetson 대비)이 결정적. 단, 출시 4개월 차라 서드파티 패키지 누락 가능 — Phase 3 게이트에서 필요한 시뮬레이터 브리지 패키지 존재 여부 확인.

**결정.** **Jazzy Jalisco** (LTS, 2024-05 출시, EOL 2029-05, Ubuntu 24.04). 추천안(Lyrical)을 뒤집고 **생태계 성숙도·자료 접근성**을 우선한 선택.

파생되는 사항:
- **rclrs 수동 설치**: 메시지 패키지(`rosidl_generator_rs` 등)와 코드 생성기를 소스에서 클론·빌드해야 함. Phase 3 착수 시 그 시점의 rclrs 버전(현재 0.7.0)이 Jazzy를 지원하는지 재확인하고, 설치 절차를 devcontainer/CI 이미지에 스크립트로 고정할 것.
- **시뮬레이터는 Gazebo Harmonic 페어링** (T13에 반영).
- **개발 환경은 Ubuntu 24.04 기준** (T1의 Linux 환경 결정 시 26.04가 아닌 24.04).
- **D16(ARM 타겟)**: Jazzy의 arm64 지원 등급을 Phase 3에서 확인할 것.
- 2029-05 EOL이므로 장기 운용 시 한 번의 배포판 이전이 필요 — 코어가 미들웨어 독립(D1)이라 이전 영향은 `rsm-ros2` 크레이트에 국한된다.

---

## T13. 시뮬레이터 (PRD D13)

| 후보 | 특징 | GPU | ROS 2 통합 |
|---|---|---|---|
| **Gazebo Jetty** | Lyrical 페어링 최신, `ros_gz` 브리지, 모바일·매니퓰레이터 표준 | 보통 사양 OK | 최고 |
| Gazebo Harmonic | 직전 LTS, 자료 풍부 | 동일 | Jazzy 페어링 |
| Isaac Sim | 고품질 렌더링·합성 데이터, ML 감지기 검증에 유리 | **강력한 NVIDIA GPU 필수** | 지원하나 학습곡선 가파름 |
| MuJoCo | 접촉 물리·휴머노이드·RL 표준, 오픈소스 | 낮음 | 네이티브 ROS 2 브리지 약함 |
| Webots | 진입 쉬움 | 낮음 | `webots_ros2` |

**추천(초안 당시).** **Gazebo Jetty** (예제 구성 (a) 매니퓰레이터·(b) 이동로봇을 먼저). (c) 이족보행은 MuJoCo가 더 적합하지만 ROS 브리지 부담이 있어 P1 이후. ML 래퍼 검증용 카메라 이미지는 Gazebo 카메라 센서로 충분(정확도 요구가 아니라 파이프라인 수용성 검증이므로).

**결정.** **Gazebo Harmonic** — T12에서 Jazzy를 택했으므로 위 표의 Jetty 행은 무효이고 Jazzy 페어링인 Harmonic을 쓴다. `ros_gz` 브리지 바이너리 사용.

**검증 순서: (b) 이동로봇 → (a) 고정 매니퓰레이터.** (b)를 먼저 하는 이유는 Gazebo+ROS 2 튜토리얼 자산이 가장 풍부해 시뮬레이터 자체와 씨름하는 시간이 최소이고, 한 구성에서 external(라이다 기반 장애물 거리·속도-거리 규칙)·internal(배터리·통신 타임아웃)·ML 래퍼(카메라)를 모두 돌릴 수 있기 때문. 이후 (a)로 넘어가 `fast` 그룹의 고주기 동작을 검증한다. **(c) 이족보행은 P1 이후** — 물리적으로는 MuJoCo가 적합하나 ROS 2 브리지 부담이 크고, (c)의 목적인 주기 그룹 혼합 검증은 관절 상태 리플레이로 대부분 달성 가능.

Isaac Sim은 v1 대상에서 제외하되, 후속에 ML 감지기를 본격적으로 다룰 때(R-20) **학습 데이터 생성 도구**로 재검토 — 감지기 검증 환경으로서가 아니므로 본 결정과 충돌하지 않는다.

---

## T14. 리플레이 기록 포맷 (PRD D15)

**개념.** R-10 리플레이 Source가 읽는 파일. 코어의 미들웨어 독립성과 ROS 생태계 재사용 사이의 선택.

| 후보 | 특징 |
|---|---|
| **자체 JSON Lines** | 코어 Signal 타입을 그대로 직렬화. 의존성 0, 사람이 읽고 손으로 만들 수 있음(테스트 케이스 작성 용이). 대용량(이미지·포인트클라우드)에 비효율 |
| MCAP 직접 읽기 (`mcap` 크레이트) | rosbag2 기본 포맷. 실기·시뮬레이터 기록을 바로 재생. 코어가 ROS 메시지 스키마(CDR 디코딩)를 알아야 함 → 독립성 훼손 |
| 하이브리드 | 코어는 JSONL(+ 바이너리 블롭 사이드카)만 읽고, `rsm-tools`가 MCAP → JSONL 변환 (`rosbags-rs` 또는 Python `rosbags`) | 독립성 유지 + 생태계 재사용 |

**추천.** **하이브리드**. v1 코어 리플레이 포맷은 JSONL(스칼라·벡터) + 대용량 신호는 파일 참조. MCAP 변환기는 `rsm-tools`(Python `rosbags` 라이브러리가 성숙).

**결정.** **코어 1차 포맷은 자체 JSON Lines**, 스칼라·벡터는 인라인, 대용량 신호(이미지·포인트클라우드)는 `"ref": "frames/000000.png"` 형태의 **파일 참조**. 손으로 타이핑해 시나리오를 만들 수 있다는 점이 R-10·T9(`insta` 스냅샷)와 맞물려 핵심 이점 — 입력과 기대 출력이 모두 diff 가능한 텍스트가 된다.

**MCAP은 변환을 강제하지 않고 별도 크레이트 `rsm-mcap`의 Source로 격리 제공**(초안의 하이브리드를 다듬은 형태). T4 레지스트리 구조상 Source는 교체 가능하므로, 크레이트 경계가 D1(코어의 미들웨어 독립)을 지켜 준다 — `rsm-core`는 MCAP/CDR/ROS 스키마를 모르는 채로 rosbag 재생이 가능해진다. v1 필수는 아니며 Phase 3에서 실기·시뮬레이터 기록을 다룰 때 추가.

보조적으로 `rsm-tools`(Python)에 `rosbags` 기반 MCAP → JSONL 변환기를 두어, 기록에서 소규모 회귀 시나리오를 추출할 때 쓴다.

크레이트 구성 갱신: `rsm-core` / `rsm-modules` / `rsm-ml` / `rsm-mcap`(선택) / `rsm-ros2` / `rsm-sys`(unsafe 격리, 필요 시) / `rsm-tools`(Python).

---

## T15. CI/CD · 크로스 빌드

| 요소 | 추천 |
|---|---|
| CI | GitHub Actions (비공개 저장소도 무료 분량 충분). 매트릭스: `x86_64-linux`, `aarch64-linux`(D16 시), Windows(코어만) |
| 잡 구성 | fmt → clippy(deny warnings) → cargo-deny → test → miri(핫패스 크레이트만, 주 1회) → llvm-cov |
| 크로스 빌드 | `cross` 크레이트(Docker 기반) 또는 `cargo-zigbuild` |
| ROS 2 잡 | Phase 3부터 `ros:jazzy` 공식 Docker 이미지 위에서 `colcon build` + rclrs |
| 릴리스 | `cargo-release` + semver, `CHANGELOG.md` (keep-a-changelog) |

**결정.** **GitHub Actions.** 비공개·공개 어느 쪽으로 D17이 결정되든 무료 분량으로 충분.

잡 구성:

| 잡 | 시점 | 내용 |
|---|---|---|
| 1. 코어 검사 | 매 푸시 | matrix `[ubuntu-latest, windows-latest]` → `cargo fmt --check` / `cargo clippy --workspace --exclude rsm-ros2 -- -D warnings` / `cargo test --workspace --exclude rsm-ros2` |
| 2. 의존성 정책 | 매 푸시 | `cargo deny check` (licenses·bans·advisories·sources) |
| 3. 커버리지 | 푸시 또는 PR | `cargo llvm-cov --workspace --exclude rsm-ros2` |
| 4. 무거운 검사 | 주 1회 또는 해당 크레이트 변경 시 | `cargo miri test -p rsm-sys`, `loom` 테스트, `criterion` 벤치 추이 |
| 5. ROS 2 | **Phase 3부터** | `ros:jazzy-ros-base` 컨테이너 + rclrs 수동 설치 스크립트 → `colcon build` / `colcon test` |

`rsm-ros2`를 잡 1~3에서 제외할 수 있는 것은 D1(코어의 미들웨어 독립) 덕분이며, CI가 빠르고 단순해지는 실질적 이점이다. 잡 5의 rclrs 설치 스크립트가 T12에서 우려한 "수동 설치 부담"의 자동화 수단이 되고, 동일 이미지를 로컬 devcontainer로 재사용하면 T1의 Phase 3 환경 결정과도 맞물린다.

**캐싱**: `Swatinem/rust-cache`(또는 동등한 `~/.cargo`·`target/` 캐시) 필수 — 없으면 매 푸시마다 의존성 전체 재빌드.

**크로스 빌드(D16 확정 반영)**: `aarch64-unknown-linux-gnu`를 **Phase 0부터 CI 매트릭스에 포함**한다(`rsm-ml` 제외). 저장소를 공개(D17)하므로 GitHub Actions의 **arm64 네이티브 러너**를 우선 시도하고, 불가하면 `cross`(Docker 기반)로 크로스 빌드한다. `ort`가 C++ ONNX Runtime을 링크하므로 `rsm-ml`만 분리한다 — 순수 Rust 부분(`rsm-core`/`rsm-modules`)은 CI에서 aarch64 빌드·테스트로 이식성을 상시 보장하고, `rsm-ml`은 **보유 ARM 보드에서 네이티브 빌드**로 Phase 2에 1회 검증한다(feature 분리라 가능). `cargo-zigbuild`는 대안으로 남겨 둔다.

**베어메탈 MCU(`no_std`)는 v1 대상이 아니다.** 주기 그룹=OS 스레드, `catch_unwind`=되감기, `Box<dyn Detector>`=힙이라는 전제가 전부 무너지므로 별개 과제(R-21/R-22 영역).

**릴리스**: `cargo-release` + semver + `CHANGELOG.md`(keep-a-changelog), **Phase 2 이후 도입**(그 전에는 릴리스 대상 없음). 도입 단계에서 잡 3·4를 붙이는 것도 같은 시점.

---

## T16. 문서 · 저장소 구조

| 요소 | 추천 |
|---|---|
| API 문서 | `rustdoc` (`cargo doc`), `#![warn(missing_docs)]` |
| 가이드 (R-11) | `mdBook` — 모듈 작성 가이드, 설정 레퍼런스(schemars 출력에서 생성), 예제 |
| 설계 기록 | `docs/adr/` — ADR(Architecture Decision Record) 형식, 본 문서의 각 결정을 ADR 1건씩으로 전환 |
| 저장소 | 단일 리포지토리 + Cargo 워크스페이스(PRD §5.3 5크레이트) + `rsm_msgs`(ROS 패키지)는 `ros/` 하위 |
| Python 도구 | `uv`로 관리, `pydantic`으로 JSON Schema 검증 |

**결정.** **API 문서 = `rustdoc`**, doctest를 CI의 `cargo test`에서 실행(문서 예제가 낡으면 빌드가 깨지도록). `#![warn(missing_docs)]` + T10의 `-D warnings`로 공개 API 문서 누락 차단.

**가이드(R-11)는 `docs/*.md`로 시작하고 mdBook 전환은 Phase 2로 미룬다** — Phase 0~1에는 담을 내용이 적어 도구 설정이 앞서는 낭비가 됨. 전환 시점에 설정 레퍼런스는 `schemars` JSON Schema에서 생성(손으로 관리하지 않음).

**설계 기록**은 당분간 본 문서 하나로 유지하고, 결정이 뒤집히기 시작하면 `docs/adr/`로 분리(결정 하나당 파일 하나: 상태·맥락·결정·근거·결과·대안).

**저장소는 단일 리포 + Cargo 워크스페이스.** 빌드 시스템이 다른 ROS 메시지 패키지(`colcon`)는 `ros/` 하위로 경계를 분리. D17에서 open-core로 가더라도 나중에 분리 가능하므로 미리 쪼개지 않는다.

**Python 도구 환경**은 `uv` + JSON Schema 검증(`pydantic` 또는 `jsonschema`). 되돌리기 쉬운 영역이라 가볍게 확정.

저장소 배치:

```
robot-safety-monitor/
├── Cargo.toml / Cargo.lock / rust-toolchain.toml / deny.toml
├── .github/workflows/          ← CI (T15)
├── rsm-core/ rsm-modules/ rsm-ml/ rsm-mcap/ rsm-ros2/ rsm-sys/
├── ros/rsm_msgs/               ← colcon 빌드
├── tools/                      ← rsm-tools (Python, uv)
├── examples/                   ← 예제 구성 YAML + 리플레이 JSONL
└── docs/  PRD.md · TECH_STACK.md · (adr/ · book/ 는 나중에)
```

---

## 요약표 (확정안)

| ID | 요소 | 확정 내용 |
|---|---|---|
| T1 | 개발 환경 | Phase 0~2 **Windows 네이티브 Rust**, Linux 환경은 Phase 3에 결정(Ubuntu 24.04 기준). 지터 공식 수치는 Linux 재측정 |
| T2 | Rust 툴체인 | **stable + edition 2024**, `rust-toolchain.toml` 버전 고정, MSRV 문서화 |
| T3 | 설정 포맷 | **YAML + `serde_yaml_ng` + `schemars`**, `deny_unknown_fields`, `serde_yml` 금지 |
| T4 | 레지스트리 | **명시적 등록 함수**, 팩토리 `fn(&Params) -> Result<Box<dyn Detector>>` 고정 |
| T5 | 스레드·큐 | **`std::thread` + `thread-priority` + `rtrb`**(전 구간 SPSC 정규화), tokio 미사용 |
| T6 | 시간 | **자체 `Instant(u64 ns)` + `trait Clock`**, `MonotonicClock`·`VirtualClock`(v1), `RosClock`(Phase 3) |
| T7 | 로깅·계측 | **3분리** — Sink `serde_json` / 진단 `tracing` / 계측 핫패스 고정 슬롯 → `metrics` |
| T8 | 에러·패닉 | **`thiserror` + `anyhow` + unwind + 모듈 tick `catch_unwind` + 패닉 모듈 재생성** |
| T9 | 테스트 | **전체 채택** `proptest`·`insta`·`loom`·Miri·`criterion`·`llvm-cov`·할당 카운터, 도입은 단계별 |
| T10 | 코딩 규칙 | core `forbid(unsafe_code)`, **`pedantic` 켬**, `indexing_slicing`은 핫패스 한정, `cargo-deny` 단일화 |
| T11 | ML 런타임 | **ONNX + `ort`**, CPU 시작·CUDA feature, `ort-tract` 대체 여지. **모델 라이선스 검토 필수(AGPL 회피)** |
| T12 | ROS 2 | **Jazzy Jalisco** (EOL 2029-05, Ubuntu 24.04). rclrs 수동 설치, Gazebo Harmonic 페어링 |
| T13 | 시뮬레이터 | **Gazebo Harmonic**, 검증 순서 **(b) 이동로봇 → (a) 매니퓰레이터**, (c)는 P1 이후 |
| T14 | 리플레이 포맷 | **코어는 JSONL**(대용량은 파일 참조), **MCAP은 `rsm-mcap` Source로 격리**, 변환기는 `rsm-tools` |
| T15 | CI | **GitHub Actions** 5개 잡(코어·deny·커버리지·주기 무거운 검사·Phase 3 ROS), `cross`로 aarch64(단 `rsm-ml` 제외) |
| T16 | 문서·구조 | **rustdoc + doctest**, 가이드는 `docs/*.md`로 시작하고 **mdBook은 Phase 2**, ADR 분리는 필요 시, 단일 리포 |

## 결정이 PRD 미결 사안에 준 답

| PRD 항목 | 상태 |
|---|---|
| D13 첫 검증 시뮬레이터 | **해결** — Gazebo Harmonic, 구성 (b) 우선 (T13) |
| D14 ML 모델·런타임 | **부분 해결** — 런타임 `ort`/ONNX 확정(T11). 구체 모델은 Phase 2에 라이선스 확인 후 선정 |
| D15 리플레이 기록 포맷 | **해결** — JSONL 코어 + `rsm-mcap` Source (T14) |
| D18 코딩 규칙 | **해결** — T10 |
| D16 타겟 환경 | **해결** — x86_64(Linux/Windows) + aarch64 Linux. CI에 aarch64 상시 포함(`rsm-ml` 제외), 실기 검증 Phase 2 1회. MCU(`no_std`)는 대상 아님 |
| D17 공개 여부·라이선스 | **해결** — 오픈소스 공개, **Apache-2.0 단독**, DCO, 공개 시점 v0.5. open-core는 지금 정하지 않음 |
| D19 투입 시간·일정 | **해결** — 주당 10시간. v0.5 약 6개월, v1 누적 11~14개월 (PRD §8) |

전건 확정으로 PRD 미결 사안은 남지 않으며, 후속 확인 항목(모델 선정·rclrs 재확인·ARM 실기·CLA 전환)은 PRD §9.2에 시점과 함께 정리되어 있다.

## 참고 자료

- rclrs 0.7.0 / 지원 배포판: https://github.com/ros2-rust/ros2_rust
- ROS 2 Lyrical Luth 출시 공지 (2026-05-22, LTS ~2031) — 배포판 비교 근거, 최종 선택은 Jazzy: https://discourse.openrobotics.org/t/ros-2-lyrical-luth-released/55021
- serde_yaml deprecated → `serde_yaml_ng` 권장, `serde_yml` 회피: https://users.rust-lang.org/t/serde-and-yaml-support-status/125684
- `ort` 2.0 rc / ONNX Runtime 1.28: https://github.com/pykeio/ort , https://ort.pyke.io/
- `rtrb` 실시간 SPSC 링버퍼: https://github.com/mgeier/rtrb
- 시뮬레이터 비교 (2026): https://www.godrift.ai/blogs/best-robot-simulators-ros2
- `rosbags-rs` (rosbag2/MCAP 리더): https://github.com/amin-abouee/rosbags-rs
