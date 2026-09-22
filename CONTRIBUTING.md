# 기여 안내

## 라이선스와 서명

이 프로젝트는 Apache License 2.0으로 배포된다. 기여는 **DCO(Developer
Certificate of Origin)** 방식으로 받는다. 커밋에 서명 줄을 붙이면 된다.

```
git commit -s -m "..."
```

위 명령이 커밋 메시지 끝에 `Signed-off-by: 이름 <메일>` 을 추가한다.
이는 "내가 이 코드를 기여할 권리가 있고, Apache-2.0으로 배포되는 데 동의한다"는
표시다. 별도의 CLA 서명 절차는 두지 않는다.

## 제출 전 체크리스트

```
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI가 같은 것을 돌린다. 로컬에서 통과시키고 올리는 편이 빠르다.

## 새 감지 모듈을 추가할 때

1. `rsm-modules/src/` 에 모듈 파일을 만들고 `Detector` 트레이트를 구현한다.
2. `rsm-modules/src/lib.rs` 의 `register_all` 에 한 줄 등록한다.
3. 리플레이 시나리오(JSONL)와 기대 이벤트 스냅샷 테스트를 추가한다.
4. 공개 항목에 문서 주석을 단다 (`missing_docs` 가 CI에서 막는다).

`rsm-core` 는 수정하지 않는다. 코어를 고쳐야 모듈이 추가된다면 그것은
인터페이스 설계 문제이니 먼저 이슈로 논의한다.

## 설계 규칙 (요약)

자세한 근거는 `docs/PRD.md` 와 `docs/TECH_STACK.md` 에 있다.

- `rsm-core` 에 `unsafe` 금지 (`forbid(unsafe_code)`).
- 코어에서 OS 시계를 직접 호출하지 않는다. `Clock` 을 주입받는다.
- 라이브러리 코드에서 `unwrap()` / `expect()` / `panic!()` 금지.
- `fast` 주기 그룹의 tick 경로에서 힙 할당·락·로깅 매크로 금지.
- 코어는 ROS 2, 특정 시뮬레이터, 특정 미들웨어를 알지 못한다.
