# CLAUDE.md

이 파일은 Claude Code가 세션 시작 시 자동으로 읽는다. 저장소에서 작업할 때의
규칙과 문맥을 담는다.

## 프로젝트

**Robot Safety Monitor (RSM)** — 로봇의 내부·외부 위험을 감지하는 SW 모듈을
표준 인터페이스로 규격화하고, **설정 파일(YAML)만으로 조합**하게 하는 Rust
프레임워크. 감지 → 판단 → 알림까지만 하고, 정지·감속 같은 대응은 로봇 제어기가 한다.

**비인증 자문(advisory) 계층이다.** 어떤 기능안전 등급에 대해서도 인증받지
않았으며 그 목적으로 사용해서는 안 된다.

## 먼저 읽을 문서

| 파일 | 언제 |
|---|---|
| `docs/STATUS.md` | **작업 시작 시 항상.** 지금 어디까지 됐는지 |
| `docs/DECISIONS.md` | 결정 요약표. "이거 왜 이렇게 돼 있지?" 할 때 |
| `docs/PRD.md` (v0.3) | 요구사항 R-01~R-24, 결정 D1~D19의 근거 |
| `docs/TECH_STACK.md` (v1.0) | 기술 선택 T1~T16의 근거 |
| `docs/CODE_TOUR.md` | 코드 읽는 순서 8정거장 + 증상→파일 표 |
| `docs/BRIEF.md` | 새 대화에 붙여 넣는 한 장 요약 |
| `private/WORKING.md` | 있으면 함께 읽는다. 개인 작업 메모이며 저장소에는 포함되지 않는다 |

## 구조

```
rsm-cli  ──▶  rsm-modules  ──▶  rsm-core
(실행기)      (감지 모듈)       (트레이트·타입·스케줄러)

화살표는 한 방향뿐이다. rsm-core 는 위 둘을 모른다.
```

| 크레이트 | 역할 |
|---|---|
| `rsm-core` | 트레이트·타입·레지스트리·설정 로더·스케줄러·Supervisor. **미들웨어 비의존** |
| `rsm-modules` | 레퍼런스 감지 모듈. 새 모듈은 `register_all`에 한 줄 추가 |
| `rsm-cli` | `rsm run｜check｜list`. JSONL 출력 |

파이프라인: `Source → Detector → Arbiter → Sink` + `Supervisor` + 주입된 `Clock`.
흐르는 타입은 셋뿐 — `Signal` / `HazardEvent` / `SafetyState`.

## 코드 규칙 (TECH_STACK T10 / PRD D18)

**어기면 빌드가 실패한다. 우회하지 말고 설계를 고칠 것.**

- `rsm-core`는 `forbid(unsafe_code)`. unsafe 가 필요하면 `rsm-sys` 크레이트로
  격리하고 `// SAFETY:` 주석으로 근거를 남긴다.
- 라이브러리 크레이트에서 `unwrap()` / `expect()` / `panic!()` 금지.
  실패는 `Result`로 돌려주고 판단은 호출자가 한다.
  테스트 모듈에서만 `#[allow(..., reason = "...")]`로 완화한다.
- **코어에서 OS 시계를 직접 호출하지 않는다.** `Clock` 트레이트를 주입받는다.
  유일한 예외는 `rsm-core::time::MonotonicClock::now` 이며 `clippy.toml`이 강제한다.
- `fast` 그룹 tick 경로에서 **힙 할당·락·로깅 매크로 금지** (R-08).
  그래서 `HazardEvent`는 `Copy`이고 이름은 전부 인터닝된 `u16` ID다.
- **코어는 ROS 2·특정 미들웨어를 알지 못한다** (PRD D1).
  `rsm-core/Cargo.toml`의 의존성 목록이 감사 지점이다.
- 코드를 제안하기 전에 **빌드·clippy·테스트를 통과시킨다.**

## 검증

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings   # 경고 0 이어야 한다
cargo test --workspace                                   # 현재 46개
cargo run -q -p rsm-cli --bin rsm -- check --config examples/demo-a.yaml
```

툴체인은 `rust-toolchain.toml`이 고정한다(1.98.1, edition 2024, MSRV 1.95).

데모 두 개가 "설정만 바꿔 다른 구성이 돈다"의 증거다. 구조를 바꿨으면 둘 다 돌려 본다.

```bash
cargo run --release -p rsm-cli --bin rsm -- run --config examples/demo-a.yaml --duration-ms 2000
cargo run --release -p rsm-cli --bin rsm -- run --config examples/demo-b.yaml --duration-ms 3000
```

`demo-b`는 일부러 통신 두절과 패닉을 일으킨다. 패닉 메시지가 찍히지만 프로세스가
죽지 않고 `availability`가 `Degraded`로 바뀌는 것이 정상 동작이다.

## 작업 후

- 결정이 바뀌면 `docs/PRD.md` 또는 `docs/TECH_STACK.md`를 갱신하고
  `docs/DECISIONS.md`·`docs/STATUS.md`도 같이 맞춘다.
- **`git push`는 사용자가 직접 한다. 커밋까지만 만든다.**
- 커밋 메시지는 한국어로, 무엇을 왜 바꿨는지 적는다.

## 공개 예정 저장소

`https://github.com/Seong-Yong-Park/robot-safety-monitor` (Apache-2.0, v0.5에 공개).

- **일정, 사업화 의도, 학습 진행 상황은 `docs/`에 쓰지 않는다.** `private/`에 둔다
  (gitignore 됨).
- 코드가 만들어 내는 문자열(에러 메시지, CLI 출력, `reason = "..."`, 크레이트
  description)은 **영어**로 쓴다. 주석은 한국어로 둔다.
- `private/`, `target/`, `Claude outputs/`는 커밋하지 않는다.

## 대화

- 한국어로 답하고 기술 용어는 영어를 병기한다.
- 순차적으로 결정할 때는 본문에 설명과 선택지를 적고 답을 기다린다.
- C/임베디드 배경이 강하므로 **C와 대비해 설명**하면 빠르다. C++은 익숙하지 않다.
