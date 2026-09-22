# 코드 투어 — 읽는 순서

이 문서는 RSM 코드를 **처음부터 끝까지 읽지 않고** 이해하기 위한 순서다.
목표는 두 가지다.

1. **찾아가기** — 증상을 보고 어느 파일을 열지 바로 안다.
2. **설명하기** — 남에게 "이게 어떻게 도는지" 3분 안에 말할 수 있다.

전부 이해할 필요는 없다. 각 정거장은 **"왜 여기 있나"** 한 문장과 **"여기가
깨지면 어떤 증상이 나오나"** 한 줄만 챙기면 통과다. 한 정거장에 30분 정도.

---

## 0정거장 — 먼저 돌려 본다 (읽기 전에)

```bash
cargo run -p rsm-cli --bin rsm -- list
cargo run -p rsm-cli --bin rsm -- check --config examples/demo-a.yaml
cargo run --release -p rsm-cli --bin rsm -- run --config examples/demo-a.yaml --duration-ms 2000
cargo run --release -p rsm-cli --bin rsm -- run --config examples/demo-b.yaml --duration-ms 3000
```

그리고 `examples/demo-a.yaml`과 `demo-b.yaml`을 **나란히 놓고 비교한다.**

- 같은 바이너리다. 코드는 한 줄도 다르지 않다.
- 그룹 수, 모듈 종류, 파라미터가 다르다.
- 출력의 `level`과 `availability`가 다르게 움직인다.

**이 프로젝트의 주장이 여기 다 들어 있다.** 나머지 정거장은 전부
"이게 어떻게 가능한가"에 대한 대답이다.

demo-b에서 콘솔에 패닉 메시지가 찍히지만 프로그램이 죽지 않는 것,
그리고 1초쯤 뒤 `availability`가 `Degraded`로 바뀌는 것을 눈으로 확인할 것.

---

## 1정거장 — 무엇이 흐르는가

**파일:** `rsm-core/src/event.rs`, `rsm-core/src/signal.rs`
**볼 것:** `Signal`, `HazardEvent`, `SafetyState` 세 타입의 필드만.

파이프라인에는 딱 세 종류가 흐른다.

| 타입 | 구간 | 한 문장 |
|---|---|---|
| `Signal` | Source → Detector | 센서에서 온 **측정값** |
| `HazardEvent` | Detector → Arbiter | 감지기 하나의 **주장** |
| `SafetyState` | Arbiter → Sink | 전부 합친 **현재 판정** |

여기서 꼭 챙길 두 가지:

- **`HazardEvent`에 `ttl`이 있다.** "위험 해제" 메시지를 따로 보내지 않는다.
  갱신이 끊기면 주장이 스스로 만료된다. 해제 메시지는 유실되거나, 모듈이 죽으면
  영영 안 오기 때문이다. C로 치면 "플래그를 내리는 코드"를 믿지 않고
  타임스탬프를 믿는 쪽이다.
- **`SafetyState`에 `availability`가 따로 있다.** `level`은 "본 것 중 최악",
  `availability`는 "얼마나 봤나". 이 둘을 분리하지 않으면 감시기가 죽은 것과
  위험이 없는 것이 같은 출력이 된다.

> **증상 → 여기**: 이벤트 필드가 비거나 이상한 값이다 / 근거(evidence) 수치가
> 안 보인다 → `event.rs`.

---

## 2정거장 — 누가 무엇을 약속하는가

**파일:** `rsm-core/src/traits.rs`
**볼 것:** `Source`, `Detector`, `ArbiterPolicy`, `Sink` 네 트레이트의 시그니처.

C로 치면 **함수 포인터 구조체(vtable)를 언어가 대신 관리해 주는 것**이다.
`Detector`를 구현하면 `configure` / `requires` / `tick` 세 함수를 반드시 제공해야
하고, 코어는 그 세 개만 부른다. 코어는 구현체가 누군지 모른다.

핵심은 `tick`의 시그니처다.

```rust
fn tick(&mut self, now: Instant, bus: &SignalBus, out: &mut dyn EventSink);
```

- `now`를 **인자로 받는다.** 감지기가 시계를 직접 부르지 않는다. 그래서 가상
  시계로 100배속 재생해도 결과가 같다.
- `out`이 `dyn EventSink`다. 감지기는 자기 이벤트가 큐로 가는지 테스트 버퍼로
  가는지 모른다.

`ModuleCtx`와 `Params`도 같이 본다. `configure` 단계에서 **문자열을 전부 정수
ID로 바꿔 받아 두는 곳**이다(인터닝). tick 경로에 문자열이 남지 않는 이유가 여기다.

> **증상 → 여기**: 새 모듈을 만드는데 무엇을 구현해야 할지 모르겠다 →
> `traits.rs`.

---

## 3정거장 — 감지기 하나를 통째로

**파일:** `rsm-modules/src/detectors.rs`의 `Threshold` (약 60줄)
**볼 것:** `configure`와 `tick` 두 함수.

가장 단순한 감지기다. 여기서 패턴을 한 번 익히면 나머지 세 개는 같은 모양이다.

- `configure`: YAML에서 `signal`·`limit`·`severity`를 읽고, 쓸 이름들을
  `ctx.signal(...)`·`ctx.hazard_type(...)`으로 **ID로 바꿔 필드에 저장**한다.
  실패하면 `Err` — 즉 **기동 자체가 실패한다.** 돌다가 조용히 아무 판정도 못 하는
  것보다 낫다.
- `tick`: 버스에서 `f64` 하나 읽고, 비교하고, 넘으면 `out.emit(...)`.
  `String`도, `Vec`도, `format!`도 없다.

그다음 `CommTimeout`을 본다. **값을 보지 않고 마지막으로 쓰인 시각만 본다.**
한 번도 오지 않은 신호도 두절로 센다. 침묵을 판정하는 책임이 이 한 모듈에
모여 있다 — 감지기마다 스테일 검사를 흩어 놓지 않는다.

> **증상 → 여기**: 특정 위험이 안 잡힌다 / 엉뚱하게 잡힌다 → 그 모듈의 `tick`.

---

## 4정거장 — YAML이 어떻게 객체가 되는가

**파일:** `rsm-core/src/registry.rs` (짧다), `rsm-core/src/config.rs`
**볼 것:** `Registry::make_detector`, `PipelineConfig::instantiate`, `build`.

여기가 "설정만으로 조합"의 기계장치다. 순서는 이렇다.

```
YAML 파싱 → 모듈 이름으로 팩토리 조회 → 인스턴스 생성 → configure()
         → declares()/requires() 로 신호 연결 검사 → GroupPlan 으로 묶기
```

`Registry`는 `HashMap<String, fn() -> Box<dyn Detector>>`다. C의
`struct { const char *name; void *(*make)(void); }` 테이블과 같다. 새 모듈은
`rsm_modules::register_all`에 한 줄 더하면 끝이고, **코어는 건드리지 않는다.**

`build`가 거부하는 것들을 훑어 둘 것 — 버전 불일치, 모듈 이름 중복, 모르는
그룹, 같은 신호를 두 Source가 냄, **아무도 내지 않는 신호를 Detector가 요구함**.
전부 기동 실패다.

> **증상 → 여기**: `rsm run`이 시작하자마자 `error:`를 뱉는다 → `config.rs`의
> 검증 부분과 `error.rs`의 메시지.

---

## 5정거장 — 한 tick에 무슨 일이 일어나는가

**파일:** `rsm-core/src/schedule.rs`
**볼 것:** `GroupWorker::tick`, 그다음 `group_loop`, 마지막에 `run`.

**`GroupWorker::tick`이 이 프로젝트에서 가장 중요한 40줄이다.**

```
now = 시계에서 한 번 읽는다          <- 이 tick의 모든 모듈이 같은 값을 본다
for source in sources: source.poll(now, &mut bus)
for detector in detectors: detector.tick(now, &bus, out)
```

여기서 설계 결정 세 개가 한꺼번에 보인다.

1. **Source 먼저, Detector 나중** — 그래서 버스에 락이 필요 없다. 한 그룹은
   스레드 하나고, 그 스레드만 자기 버스를 만진다.
2. **`now`를 한 번만 읽는다** — 판정이 모듈 실행 순서에 흔들리지 않는다.
3. **각 호출이 `catch_unwind`로 감싸여 있다** — 모듈이 패닉해도 그 모듈만
   `Faulted`로 빠지고 다음 tick부터 건너뛴다. 나머지는 계속 돈다.

`group_loop`는 **절대 데드라인** 방식이다. `deadline += period`로 다음 시각을
정하고 그때까지 잔다. `deadline = now + period`로 하면 매 tick마다 실행 시간이
누적되어 주기가 밀린다. 이미 지나쳤으면 밀린 만큼 따라잡지 않고 위상을
현재로 맞춘다(따라잡기를 하면 늦은 뒤에 tick이 몰아쳐 더 나빠진다).

`QueueSink`도 본다. **큐가 가득 차면 이벤트를 버린다.** 여기서 블로킹하면
1 kHz 그룹이 Arbiter 속도에 묶인다. 대신 오버플로를 기록하고 Supervisor가
`Degraded`로 알린다 — 주기를 지키되 "빠뜨린 게 있다"는 사실은 숨기지 않는다.

> **증상 → 여기**: 주기가 안 맞는다 / 지터가 크다 / 이벤트가 새는 것 같다 →
> `schedule.rs`. `rsm run`이 끝날 때 stderr에 찍는 `tick / overrun / overflow /
> max / mean`이 첫 단서다.

---

## 6정거장 — 이벤트가 어떻게 상태가 되는가

**파일:** `rsm-core/src/arbiter.rs`, `rsm-cli/src/jsonl.rs`
**볼 것:** `MaxSeverity::ingest`와 `evaluate`, 그리고 `JsonlSink::publish`.

`MaxSeverity`는 딱 세 가지를 한다.

1. 같은 위험 타입은 **최신 관측으로 덮어쓴다** (오래된 관측이 새 관측을 이기지 않게).
2. 평가할 때 **만료된 이벤트를 버린다** (TTL).
3. 남은 것 중 **최고 심각도**가 `level`이 된다.

정책이 트레이트(`ArbiterPolicy`)인 이유는, 나중에 히스테리시스나 가중 융합을
넣을 때 코어를 안 고치기 위해서다.

`JsonlSink::publish`는 **정수 ID가 다시 문자열이 되는 유일한 지점**이다.
`names.hazards.name(id)` — 인터닝의 역방향. 이 한 곳을 보면 1정거장의
"tick 경로에 문자열이 없다"가 왜 성립하는지 닫힌다.

> **증상 → 여기**: 위험이 잡혔는데 출력에 안 나온다 / 너무 오래 남아 있다 →
> `arbiter.rs`. 출력 형식·필드 이름 문제 → `jsonl.rs`.

---

## 7정거장 — 조용함을 어떻게 다루는가

**파일:** `rsm-core/src/supervisor.rs`
**볼 것:** `HealthRegistry::scan`.

감시기 자신을 감시하는 곳이다. 판정 규칙은 `scan` 한 함수에 다 있다.

| 조건 | 결과 |
|---|---|
| 어떤 그룹이 `group_stale_after`보다 오래 tick을 안 했다 | `Unavailable` |
| 큐 오버플로가 있었다 / 고장난 모듈이 있다 | `Degraded` |
| 첫 tick 전 | 판정하지 않음 |

**모든 필드가 원자 변수다.** Supervisor는 큐를 쓰지 않는다. 그룹 스레드가 tick
끝에 카운터를 올리고, Supervisor가 주기적으로 읽는다. 큐를 쓰면 큐가 막혔을 때
건강 정보도 같이 막히는데, 그건 정확히 알아야 할 때 못 알게 되는 구조다.

TTL(6정거장)과 Supervisor는 **독립적인 두 장치**다. TTL은 "이 주장이 낡았다",
Supervisor는 "감시기가 못 보고 있다". 둘 다 있어야 침묵이 안전으로 읽히지 않는다.

> **증상 → 여기**: `availability`가 계속 `Degraded`다 / 기대와 다르게 `Available`
> 이다 → `scan`의 임계값과 `demo-*.yaml`의 `group_stale_after_ms`.

---

## 8정거장 — 왜 시계를 주입하는가

**파일:** `rsm-core/src/time.rs`
**볼 것:** `trait Clock`, `MonotonicClock`, `VirtualClock`.

`MonotonicClock::now`가 **코어 전체에서 `std::time::Instant::now()`를 부르는
유일한 곳**이다. `clippy.toml`의 `disallowed-methods`가 나머지를 막는다.
규칙을 문서가 아니라 린트로 강제한 것이다.

그 대가로 얻는 것: `rsm-modules/tests/pipeline.rs`를 보면 스레드를 하나도 띄우지
않고 `clock.advance(period)`로 tick을 손으로 밟는다. CI에서 결과가 흔들리지 않고,
나중에 JSONL 리플레이도 같은 원리로 붙는다.

> **증상 → 여기**: 테스트가 가끔 실패한다 / 시각이 이상하다 → `time.rs`,
> 그리고 누가 `now`를 직접 부르고 있지는 않은지.

---

## 부록 A — 증상에서 파일로 (한 장 요약)

| 증상 | 첫 번째로 열 파일 |
|---|---|
| 기동하자마자 `error:` | `rsm-core/src/config.rs` → `error.rs` |
| 모르는 모듈 타입이라고 한다 | `rsm-modules/src/lib.rs`의 `register_all` |
| 특정 위험이 안 잡힌다 | 해당 모듈의 `tick` (`rsm-modules/src/detectors.rs`) |
| 파라미터가 안 먹는다 | 해당 모듈의 `configure` + `traits.rs`의 `Params` |
| 잡혔는데 출력에 안 나온다 | `rsm-core/src/arbiter.rs` (TTL 만료?) |
| 너무 오래 남아 있다 | 그 모듈의 `ttl_ms` 설정 → `arbiter.rs` |
| 출력 필드·형식 | `rsm-cli/src/jsonl.rs` |
| 주기가 밀린다 / 지터 | `rsm-core/src/schedule.rs`의 `group_loop` |
| 이벤트가 새는 것 같다 | `QueueSink` 오버플로 + `queue_capacity` 설정 |
| `availability`가 이상하다 | `rsm-core/src/supervisor.rs`의 `scan` |
| 프로세스가 통째로 죽었다 | `catch_unwind` 바깥 — `schedule.rs`, `main.rs` |
| 테스트가 가끔 실패한다 | `time.rs` / 누가 OS 시계를 직접 부르는지 |

---

## 부록 B — 남에게 설명하는 3분 스크립트

> RSM은 로봇의 위험 감지 모듈을 **표준 인터페이스로 규격화**하고 **설정 파일만으로
> 조합**하게 하는 프레임워크입니다. 감지·판단·알림까지만 하고, 정지나 감속은
> 로봇 제어기가 합니다.
>
> 파이프라인은 네 단계예요. **Source**가 센서 값을 읽어 신호 버스에 쓰고,
> **Detector**가 그걸 보고 위험을 주장하고, **Arbiter**가 주장들을 합쳐 하나의
> 상태로 만들고, **Sink**가 내보냅니다. 이 네 단계를 YAML 한 장이 선언합니다.
> 구성이 달라져도 바이너리는 다시 빌드하지 않습니다.
>
> 실행은 **주기 그룹** 단위입니다. 관절 한계처럼 빨리 봐야 하는 건 1 kHz 그룹,
> 거리 센서처럼 느린 건 100 Hz 그룹. 그룹 하나가 스레드 하나고, 그룹 안에서는
> Source를 다 돌린 뒤 Detector를 돌리기 때문에 **락이 필요 없습니다.** 그룹 밖으로
> 나가는 건 위험 이벤트뿐이고, wait-free SPSC 링버퍼로 갑니다.
>
> 설계에서 제일 신경 쓴 건 **"조용한 것"이 "안전한 것"으로 읽히지 않게** 하는
> 겁니다. 장치가 두 개 있어요. 하나는 이벤트마다 붙은 **TTL** — 해제 메시지를
> 보내는 대신 주장이 스스로 만료됩니다. 다른 하나는 **Supervisor** — 모듈이
> 죽었거나 그룹이 멈췄으면 출력의 `availability`를 `Degraded`나 `Unavailable`로
> 바꿉니다. 그래서 소비자는 위험도와 "얼마나 봤나"를 항상 같이 받습니다.
>
> 모듈 하나가 패닉해도 전체가 죽지 않습니다. 모듈 호출마다 `catch_unwind`로
> 감싸서, 고장난 모듈만 빼고 계속 돕니다.
>
> 핫패스에는 **힙 할당이 없습니다.** 모듈·위험 타입 이름은 설정을 읽을 때 전부
> 정수 ID로 바꿔 두고(인터닝), 문자열은 입구인 YAML과 출구인 JSON 출력에만
> 있습니다.
>
> 그리고 코어는 **OS 시계를 직접 부르지 않습니다.** `Clock` 트레이트를 주입받아서,
> 가상 시계로 tick을 밟으면 테스트와 리플레이가 결정적으로 재현됩니다. 이 규칙은
> clippy 린트로 강제돼 있습니다.
>
> 한계도 분명히 해 둡니다. 일반 OS·아웃오브오더 CPU에서 도는 **비인증 자문
> 계층**이에요. 지터 수치는 통계적 관측이지 WCET 보장이 아닙니다. 최종 안전
> 책임은 기존 안전 회로에 있습니다.

---

## 부록 C — 자가 점검 5문

막힘없이 대답되면 투어는 끝이다.

1. 감지기가 "위험 해제"를 보내지 않는데, 위험 상태는 어떻게 사라지는가?
2. 그룹 안에서 신호 버스에 락이 필요 없는 이유는?
3. 큐가 가득 차면 왜 기다리지 않고 버리는가? 버린 사실은 어떻게 알려지는가?
4. `level: NONE`이 "안전하다"는 뜻이 **아닐 수 있는** 경우는?
5. 1 kHz tick 경로에 `String`이 하나도 없는 이유는? 그럼 사람이 읽는 이름은 어디서 나오나?

<details>
<summary>답 요약</summary>

1. 이벤트마다 TTL이 있고, Arbiter가 평가할 때 만료된 것을 버린다. 감지기가 계속
   주장을 갱신하지 않으면 저절로 사라진다.
2. 그룹 하나 = 스레드 하나이고, 그 그룹의 버스는 그 스레드만 만진다. 한 tick은
   Source 전부 → Detector 전부 순서라 쓰기와 읽기가 겹치지 않는다.
3. 블로킹하면 빠른 그룹이 느린 Arbiter에 묶여 주기를 놓친다. 대신 `QueueSink`가
   오버플로를 기록하고 Supervisor가 `availability`를 `Degraded`로 내린다.
4. `availability`가 `Degraded`/`Unavailable`일 때. 못 본 것이지 없는 것이 아니다.
5. 이름은 설정 로드 때 `Interner`가 `u16`으로 바꿔 두고, tick은 ID만 다룬다.
   사람이 읽는 이름은 출력 직전 `JsonlSink::publish`에서 역변환표로 되돌린다.

</details>
