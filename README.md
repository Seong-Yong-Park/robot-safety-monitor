# Robot Safety Monitor (RSM)

로봇의 **내부**(관절 한계, 토크, 온도, 배터리, 통신 타임아웃)와 **외부**(사람
근접, 장애물 거리, 속도-거리 규칙) 위험을 감지하는 SW 모듈을 표준 인터페이스로
규격화하고, **설정 파일만으로 조합**할 수 있게 하는 프레임워크.

감지 결과는 일관된 스키마의 위험 이벤트로 발행한다. 실제 정지·감속 같은 대응
동작은 로봇 측 제어기에 맡긴다.

> **상태: Phase 0 완료.** 설정 한 장으로 파이프라인이 조립되어 돌아간다.
> 설계 문서는 [`docs/PRD.md`](docs/PRD.md), 기술 선택의 근거는
> [`docs/TECH_STACK.md`](docs/TECH_STACK.md)에 있다.

> **비인증 자문(advisory) 계층이다.** 안전 정지의 최종 책임은 로봇의 기존
> 안전 회로와 제어기에 있다. 이 소프트웨어는 어떤 기능안전 등급에 대해서도
> 인증받지 않았으며, 그 목적으로 사용해서는 안 된다.

## 해 보기

```bash
cargo run -p rsm-cli --bin rsm -- list
cargo run -p rsm-cli --bin rsm -- check --config examples/demo-a.yaml
cargo run --release -p rsm-cli --bin rsm -- run --config examples/demo-a.yaml --duration-ms 2000
```

`demo-a.yaml`(팔, 2개 주기 그룹)과 `demo-b.yaml`(이동로봇, 고장 시연)은
**같은 바이너리**가 읽는다. 조합이 달라져도 코드는 바뀌지 않는다.

출력은 JSON Lines 한 줄에 상태 하나다.

```json
{"ts_ns":250860890,"level":"CRITICAL","availability":"Available","active":[
  {"source":"torque_limit","kind":"internal.joint.torque_limit","category":"internal",
   "severity":"CRITICAL","confidence":1.0,"ttl_ms":50,
   "evidence":{"measured":114.5,"limit":100.0}}]}
```

`level`만 보지 말고 `availability`를 함께 봐야 한다. `Degraded`/`Unavailable`
일 때의 `level`은 "위험이 없다"는 뜻이 아니라 "다 못 봤다"는 뜻이다.

## 구조

```
 Sources ──▶ Detectors ──▶ Arbiter ──▶ Sinks
 (입력      (internal.* /   (융합·판단)   (JSON Lines·
  어댑터)    external.* /                  stdout·ROS 토픽)
             ml.*)
        + Supervisor : 모듈 생존·입력 스테일 감시
        + Clock      : 외부 주입 (실기 / 리플레이 / 시뮬레이션)
```

4단계 모두를 YAML 한 파일이 선언한다. 같은 바이너리로 서로 다른 감시 구성을
돌리는 것이 이 프로젝트의 핵심 주장이다.

## 크레이트

| 크레이트 | 역할 |
|---|---|
| `rsm-core` | 트레이트·타입·레지스트리·설정 로더·스케줄러·Supervisor. **미들웨어 비의존** |
| `rsm-modules` | 레퍼런스 감지 모듈 (`ramp`·`constant`·`stalling` / `threshold`·`min_distance`·`comm_timeout`·`panic_probe`) |
| `rsm-cli` | `rsm` 실행기 — `run` / `check` / `list` |
| `rsm-ml` (예정) | 사전학습 ONNX 모델 래퍼 Detector |
| `rsm-mcap` (예정) | rosbag2/MCAP 리플레이 Source |
| `rsm-ros2` (예정) | rclrs 기반 Source/Sink 어댑터 |

## 개발

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps --open
```

툴체인은 `rust-toolchain.toml`이 고정한다(현재 1.98.1, edition 2024).
코딩 규칙은 CI에서 강제된다 — 자세한 내용은 [CONTRIBUTING.md](CONTRIBUTING.md).

## 공개 전 할 일

- [ ] `LICENSE`를 Apache-2.0 정본으로 교체
- [ ] `Cargo.toml`의 `repository` URL 확정
- [ ] v0.5 범위 완료 (코어 + 레퍼런스 모듈 3종 + JSONL 리플레이)

## 라이선스

Apache License 2.0. [LICENSE](LICENSE) 참조.
