//! 리플레이 Source — 적어 둔 신호를 시각에 맞춰 다시 내보낸다 (R-10, PRD D15·D28).
//!
//! 실기 없이 "이 구성이 이 상황에서 언제 무엇을 내는가"를 확인하는 입구다.
//! [`schedule::replay`](rsm_core::schedule::replay)의 가상 시각과 같이 쓰면 결과가 실행
//! 속도와 무관하게 늘 같다 — 회귀 시험의 기준선(스냅샷)이 된다.
//!
//! # 파일 형식 (JSON Lines)
//!
//! 한 줄에 표본 하나.
//!
//! ```text
//! # 0.5 초에 토크가 한계를 넘는다
//! {"t_ms": 0,   "signal": "joint.torque", "value": 40.0}
//! {"t_ms": 500, "signal": "joint.torque", "value": 110.0}
//! {"t_ms": 500, "signal": "door.open",    "value": true}
//! {"t_ms": 520, "signal": "joint.pos",    "value": [0.1, 0.2, 0.3]}
//! ```
//!
//! | 키 | 뜻 |
//! |---|---|
//! | `t_ms` | 재생 시작(첫 tick)부터의 시각, ms. 소수 허용. **줄 순서대로 같거나 커야 한다** |
//! | `signal` | 신호 이름 |
//! | `value` | 숫자 → 스칼라, `true`/`false` → 불리언, 숫자 배열 → 벡터(최대 [`MAX_VECTOR`]개) |
//!
//! - 빈 줄과 `#`로 시작하는 줄은 건너뛴다. 손으로 시나리오를 쓸 때 설명을 단다.
//! - 다른 키가 있으면 기동 실패다. 오타가 조용히 무시되면 시나리오가 뜻과 다르게 돈다.
//! - 파일이 끝나면 더는 쓰지 않는다. 그 신호는 낡아 가고, `comm_timeout`이 있으면 끊김으로
//!   잡힌다 — 실제로 센서가 멎은 것과 같다.
//!
//! | 파라미터 | 기본값 | 뜻 |
//! |---|---|---|
//! | `path` | (필수) | 입력 파일. 상대 경로는 설정 파일이 있는 디렉터리 기준 |

use rsm_core::error::ConfigError;
use rsm_core::signal::{FixedVec, MAX_VECTOR, Signal, SignalBus, SignalId};
use rsm_core::time::{Duration, Instant};
use rsm_core::traits::{ModuleCtx, Source};
use serde::Deserialize;

/// 파일의 한 줄.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    t_ms: f64,
    signal: String,
    value: serde_json::Value,
}

/// 표본 하나: 재생 시작으로부터의 시각, 신호, 값.
type Sample = (Duration, SignalId, Signal);

/// `t_ms`의 상한. 약 11.5일 — 기록 한 편으로는 충분하고, ns 로 바꿔도 `u64`에 넉넉하다.
const MAX_T_MS: f64 = 1e9;

/// 리플레이 Source. 파일은 `configure` 때 통째로 읽는다. `poll`은 커서만 옮기므로
/// 할당이 없다 (R-08).
#[derive(Debug, Default)]
pub struct Replay {
    /// 파일에 나오는 신호 이름(첫 등장 순, 중복 없음).
    declared: Vec<String>,
    samples: Vec<Sample>,
    /// 다음에 낼 표본의 위치.
    next: usize,
    /// 첫 `poll`의 시각. 파일의 `t_ms`는 이 시각 기준이다.
    start: Option<Instant>,
}

impl Replay {
    /// 파일 내용을 표본으로 바꾼다. 오류 메시지에 줄 번호를 붙인다.
    fn load(&mut self, ctx: &mut ModuleCtx<'_>, text: &str, shown: &str) -> Result<(), String> {
        let mut prev = Duration::ZERO;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let at = |msg: String| format!("{shown}:{}: {msg}", i + 1);
            let l: Line = serde_json::from_str(line).map_err(|e| at(e.to_string()))?;
            let t = millis(l.t_ms).ok_or_else(|| {
                at(format!(
                    "t_ms must be within 0..={MAX_T_MS}, got {}",
                    l.t_ms
                ))
            })?;
            if t < prev {
                return Err(at(format!("t_ms {} goes backwards", l.t_ms)));
            }
            prev = t;
            let value = to_signal(&l.value).map_err(at)?;
            let id = ctx.signal(&l.signal).map_err(|e| at(e.to_string()))?;
            if !self.declared.contains(&l.signal) {
                self.declared.push(l.signal);
            }
            self.samples.push((t, id, value));
        }
        if self.samples.is_empty() {
            return Err(format!("{shown}: no samples"));
        }
        Ok(())
    }
}

impl Source for Replay {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        let path = ctx.params().str("path")?.to_owned();
        let full = ctx.resolve_path(&path);
        let module = ctx.module_name().to_owned();
        let text = std::fs::read_to_string(&full).map_err(|e| {
            ConfigError::Invalid(format!(
                "module `{module}`: cannot read replay file `{}`: {e}",
                full.display()
            ))
        })?;
        self.load(ctx, &text, &path)
            .map_err(|msg| ConfigError::Invalid(format!("module `{module}`: {msg}")))
    }

    fn declares(&self) -> Vec<String> {
        self.declared.clone()
    }

    fn poll(&mut self, now: Instant, bus: &mut SignalBus) {
        let start = *self.start.get_or_insert(now);
        while let Some(&(t, id, value)) = self.samples.get(self.next) {
            let at = start + t;
            if at > now {
                break;
            }
            // 표본에 적힌 시각으로 쓴다. 두 tick 사이에 여러 표본이 있으면 마지막 값이 남고,
            // 신호의 "마지막 갱신 시각"은 tick 시각이 아니라 실제 표본 시각이 된다.
            bus.write(id, value, at);
            self.next += 1;
        }
    }
}

/// ms(실수)를 [`Duration`]으로. 범위를 벗어나거나 유한하지 않으면 `None`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked finite and within 0..=1e9 ms just above, so the ns value fits u64"
)]
fn millis(ms: f64) -> Option<Duration> {
    if ms.is_finite() && (0.0..=MAX_T_MS).contains(&ms) {
        Some(Duration::from_nanos((ms * 1_000_000.0).round() as u64))
    } else {
        None
    }
}

/// JSON 값을 신호로.
fn to_signal(v: &serde_json::Value) -> Result<Signal, String> {
    match v {
        serde_json::Value::Bool(b) => Ok(Signal::Bool(*b)),
        serde_json::Value::Number(n) => n
            .as_f64()
            .map(Signal::Scalar)
            .ok_or_else(|| format!("value {n} is not a finite number")),
        serde_json::Value::Array(items) => {
            if items.len() > MAX_VECTOR {
                return Err(format!(
                    "a vector holds at most {MAX_VECTOR} values, got {}",
                    items.len()
                ));
            }
            let mut out = FixedVec::new();
            for x in items {
                let f = x
                    .as_f64()
                    .ok_or_else(|| format!("vector element {x} is not a number"))?;
                out.push(f);
            }
            Ok(Signal::Vector(out))
        }
        other => Err(format!(
            "value must be a number, a boolean or an array of numbers, got {other}"
        )),
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::Replay;
    use rsm_core::event::ModuleId;
    use rsm_core::intern::Names;
    use rsm_core::signal::{Signal, SignalBus, SignalId};
    use rsm_core::time::{Duration, Instant};
    use rsm_core::traits::{ModuleCtx, Params, Source};

    /// 문자열을 파일 대신 읽힌다. 오류면 메시지를 돌려준다.
    fn load(text: &str) -> Result<(Replay, Names), String> {
        let p = Params::default();
        let mut names = Names::new();
        let mut r = Replay::default();
        {
            let mut ctx = ModuleCtx::new(ModuleId(0), "t".to_owned(), &p, &mut names);
            r.load(&mut ctx, text, "s.jsonl")?;
        }
        Ok((r, names))
    }

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn samples_come_out_when_their_time_arrives() {
        let (mut r, _) = load(
            "# 주석과 빈 줄은 건너뛴다\n\n\
             {\"t_ms\": 0, \"signal\": \"x\", \"value\": 1.0}\n\
             {\"t_ms\": 2, \"signal\": \"x\", \"value\": 2.0}\n\
             {\"t_ms\": 2.5, \"signal\": \"x\", \"value\": 3.0}\n",
        )
        .unwrap();
        assert_eq!(r.declares(), vec!["x".to_owned()]);
        let mut bus = SignalBus::with_capacity(1);
        let start = Instant::from_nanos(1_000_000_000); // 재생 시작 시각은 아무 값이어도 된다
        let mut seen = Vec::new();
        for k in 0..4u64 {
            r.poll(start + MS * k, &mut bus);
            let slot = bus.slot(SignalId(0)).copied().unwrap();
            seen.push((bus.read_f64(SignalId(0)).unwrap(), slot.updated_at - start));
        }
        assert_eq!(
            seen,
            vec![
                (1.0, Duration::ZERO),
                (1.0, Duration::ZERO), //               1 ms: 아직 다음 표본 전
                (2.0, MS * 2),         //               2 ms
                (3.0, Duration::from_micros(2_500)), // 3 ms tick 에 2.5 ms 표본 — 시각은 표본 것
            ]
        );
    }

    #[test]
    fn booleans_and_vectors() {
        let (mut r, names) = load(
            "{\"t_ms\": 0, \"signal\": \"door\", \"value\": true}\n\
             {\"t_ms\": 0, \"signal\": \"q\", \"value\": [0.5, 1.5]}\n",
        )
        .unwrap();
        let mut bus = SignalBus::with_capacity(names.signals.len());
        r.poll(Instant::ZERO, &mut bus);
        let door = bus.slot(SignalId(0)).copied().unwrap().value;
        assert_eq!(door, Some(Signal::Bool(true)));
        let Some(Signal::Vector(q)) = bus.slot(SignalId(1)).copied().unwrap().value else {
            panic!("q must be a vector");
        };
        assert_eq!(q.as_slice(), &[0.5, 1.5]);
    }

    #[test]
    fn errors_carry_the_line_number() {
        let bad =
            "{\"t_ms\": 0, \"signal\": \"x\", \"value\": 1}\n{\"t_ms\": 1, \"signal\": \"x\"}\n";
        let err = load(bad).unwrap_err();
        assert!(err.starts_with("s.jsonl:2:"), "{err}");
    }

    #[test]
    fn time_must_not_go_backwards() {
        let err = load(
            "{\"t_ms\": 5, \"signal\": \"x\", \"value\": 1}\n\
             {\"t_ms\": 4, \"signal\": \"x\", \"value\": 1}\n",
        )
        .unwrap_err();
        assert!(err.contains("backwards"), "{err}");
    }

    #[test]
    fn typos_and_bad_values_are_rejected() {
        // 키 오타
        assert!(load("{\"t_ms\": 0, \"signl\": \"x\", \"value\": 1}").is_err());
        // 음수 시각
        assert!(load("{\"t_ms\": -1, \"signal\": \"x\", \"value\": 1}").is_err());
        // 문자열 값
        assert!(load("{\"t_ms\": 0, \"signal\": \"x\", \"value\": \"hi\"}").is_err());
        // 빈 파일
        assert!(
            load("# only a comment\n")
                .unwrap_err()
                .contains("no samples")
        );
    }
}
