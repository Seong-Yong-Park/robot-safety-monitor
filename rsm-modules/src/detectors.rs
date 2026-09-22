//! 레퍼런스 Detector 모듈.
//!
//! 모든 Detector는 같은 계약을 지킨다.
//!
//! * `configure` 에서 파라미터를 읽고 쓸 이름을 전부 인터닝해 정수 ID로 받아 둔다.
//! * `tick` 에서는 문자열을 만들지 않고 힙도 건드리지 않는다 (R-08).
//! * 판정은 **TTL 붙은 주장**으로만 낸다. 해제 메시지는 보내지 않는다 —
//!   갱신이 끊기면 스스로 만료되는 쪽이 안전하다.

use rsm_core::error::ConfigError;
use rsm_core::event::{Category, EvidenceKey, HazardEvent, HazardTypeId, ModuleId, Severity};
use rsm_core::signal::{SignalBus, SignalId};
use rsm_core::time::{Duration, Instant};
use rsm_core::traits::{Detector, EventSink, Health, ModuleCtx};

use crate::{count_param, millis_to_duration};

/// 설정 문자열을 [`Severity`]로 바꾼다.
fn severity_from(name: &str, module: &str) -> Result<Severity, ConfigError> {
    match name {
        "advisory" => Ok(Severity::Advisory),
        "warning" => Ok(Severity::Warning),
        "critical" => Ok(Severity::Critical),
        _ => Err(ConfigError::BadParam {
            module: module.to_owned(),
            key: "severity".to_owned(),
            expected: "advisory | warning | critical",
        }),
    }
}

/// 한계값 초과·미달 감지기.
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `signal` | (필수) | 감시할 신호 이름 |
/// | `limit` | (필수) | 한계값 |
/// | `direction` | `above` | `above` 면 초과, `below` 면 미달에서 발화 |
/// | `hazard` | `internal.threshold` | 위험 타입 이름 |
/// | `severity` | `warning` | 심각도 |
/// | `ttl_ms` | `200` | 이 주장이 유효한 시간 |
#[derive(Debug, Default)]
pub struct Threshold {
    signal_name: String,
    signal: SignalId,
    me: ModuleId,
    kind: HazardTypeId,
    k_measured: EvidenceKey,
    k_limit: EvidenceKey,
    limit: f64,
    above: bool,
    severity: Severity,
    ttl: Duration,
}

impl Detector for Threshold {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        let module = ctx.module_name().to_owned();
        self.signal_name = ctx.params().str("signal")?.to_owned();
        self.limit = ctx.params().f64("limit")?;
        self.above = match ctx.params().str_or("direction", "above")? {
            "above" => true,
            "below" => false,
            _ => {
                return Err(ConfigError::BadParam {
                    module,
                    key: "direction".to_owned(),
                    expected: "above | below",
                });
            }
        };
        let hazard = ctx
            .params()
            .str_or("hazard", "internal.threshold")?
            .to_owned();
        let sev = ctx.params().str_or("severity", "warning")?.to_owned();
        self.severity = severity_from(&sev, &module)?;
        self.ttl = millis_to_duration(ctx.params().f64_or("ttl_ms", 200.0)?);

        self.me = ctx.module_id();
        self.kind = ctx.hazard_type(&hazard)?;
        self.k_measured = ctx.evidence_key("measured")?;
        self.k_limit = ctx.evidence_key("limit")?;
        self.signal = ctx.signal(&self.signal_name)?;
        Ok(())
    }

    fn requires(&self) -> Vec<String> {
        vec![self.signal_name.clone()]
    }

    fn tick(&mut self, now: Instant, bus: &SignalBus, out: &mut dyn EventSink) {
        let Some(v) = bus.read_f64(self.signal) else {
            // 값이 아직 없다. 여기서 "안전"이라고 단정하지 않는다 —
            // 침묵을 판정하는 것은 `comm_timeout`과 Supervisor의 일이다.
            return;
        };
        let hit = if self.above {
            v > self.limit
        } else {
            v < self.limit
        };
        if hit {
            out.emit(
                HazardEvent::new(
                    self.me,
                    self.kind,
                    Category::Internal,
                    self.severity,
                    now,
                    self.ttl,
                )
                .with_evidence(self.k_measured, v)
                .with_evidence(self.k_limit, self.limit),
            );
        }
    }
}

/// 최소 거리 감지기. 경고 거리와 위험 거리 두 단계를 갖는다.
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `signal` | (필수) | 거리 신호 이름 (미터) |
/// | `warn_m` | `1.0` | 이 아래면 `warning` |
/// | `critical_m` | `0.5` | 이 아래면 `critical` |
/// | `hazard` | `external.min_distance` | 위험 타입 이름 |
/// | `ttl_ms` | `200` | 주장 유효 시간 |
#[derive(Debug, Default)]
pub struct MinDistance {
    signal_name: String,
    signal: SignalId,
    me: ModuleId,
    kind: HazardTypeId,
    k_distance: EvidenceKey,
    k_threshold: EvidenceKey,
    warn_m: f64,
    critical_m: f64,
    ttl: Duration,
}

impl Detector for MinDistance {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        let module = ctx.module_name().to_owned();
        self.signal_name = ctx.params().str("signal")?.to_owned();
        self.warn_m = ctx.params().f64_or("warn_m", 1.0)?;
        self.critical_m = ctx.params().f64_or("critical_m", 0.5)?;
        if self.critical_m >= self.warn_m {
            return Err(ConfigError::BadParam {
                module,
                key: "critical_m".to_owned(),
                expected: "a value smaller than `warn_m`",
            });
        }
        let hazard = ctx
            .params()
            .str_or("hazard", "external.min_distance")?
            .to_owned();
        self.ttl = millis_to_duration(ctx.params().f64_or("ttl_ms", 200.0)?);

        self.me = ctx.module_id();
        self.kind = ctx.hazard_type(&hazard)?;
        self.k_distance = ctx.evidence_key("distance_m")?;
        self.k_threshold = ctx.evidence_key("threshold_m")?;
        self.signal = ctx.signal(&self.signal_name)?;
        Ok(())
    }

    fn requires(&self) -> Vec<String> {
        vec![self.signal_name.clone()]
    }

    fn tick(&mut self, now: Instant, bus: &SignalBus, out: &mut dyn EventSink) {
        let Some(d) = bus.read_f64(self.signal) else {
            return;
        };
        let (severity, threshold) = if d < self.critical_m {
            (Severity::Critical, self.critical_m)
        } else if d < self.warn_m {
            (Severity::Warning, self.warn_m)
        } else {
            return;
        };
        out.emit(
            HazardEvent::new(
                self.me,
                self.kind,
                Category::External,
                severity,
                now,
                self.ttl,
            )
            .with_evidence(self.k_distance, d)
            .with_evidence(self.k_threshold, threshold),
        );
    }
}

/// 신호 갱신이 끊겼는지 보는 감지기 (R-16).
///
/// 값 자체는 보지 않는다. **마지막으로 쓰인 시각**만 본다. 한 번도 쓰이지 않은
/// 신호도 두절로 센다. "조용함"을 안전으로 읽지 않기 위한 모듈이다.
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `signal` | (필수) | 감시할 신호 이름 |
/// | `timeout_ms` | `100` | 이보다 오래 갱신이 없으면 발화 |
/// | `hazard` | `internal.comm_timeout` | 위험 타입 이름 |
/// | `severity` | `critical` | 심각도 |
/// | `ttl_ms` | `500` | 주장 유효 시간 |
#[derive(Debug, Default)]
pub struct CommTimeout {
    signal_name: String,
    signal: SignalId,
    me: ModuleId,
    kind: HazardTypeId,
    k_age: EvidenceKey,
    k_limit: EvidenceKey,
    timeout: Duration,
    severity: Severity,
    ttl: Duration,
}

impl Detector for CommTimeout {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        let module = ctx.module_name().to_owned();
        self.signal_name = ctx.params().str("signal")?.to_owned();
        self.timeout = millis_to_duration(ctx.params().f64_or("timeout_ms", 100.0)?);
        let hazard = ctx
            .params()
            .str_or("hazard", "internal.comm_timeout")?
            .to_owned();
        let sev = ctx.params().str_or("severity", "critical")?.to_owned();
        self.severity = severity_from(&sev, &module)?;
        self.ttl = millis_to_duration(ctx.params().f64_or("ttl_ms", 500.0)?);

        self.me = ctx.module_id();
        self.kind = ctx.hazard_type(&hazard)?;
        self.k_age = ctx.evidence_key("age_ms")?;
        self.k_limit = ctx.evidence_key("timeout_ms")?;
        self.signal = ctx.signal(&self.signal_name)?;
        Ok(())
    }

    fn requires(&self) -> Vec<String> {
        vec![self.signal_name.clone()]
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "evidence numbers are for humans; millisecond precision is enough"
    )]
    fn tick(&mut self, now: Instant, bus: &SignalBus, out: &mut dyn EventSink) {
        let Some(slot) = bus.slot(self.signal) else {
            return;
        };
        if !slot.is_stale(now, self.timeout) {
            return;
        }
        let age = now.saturating_duration_since(slot.updated_at);
        out.emit(
            HazardEvent::new(
                self.me,
                self.kind,
                Category::Internal,
                self.severity,
                now,
                self.ttl,
            )
            .with_evidence(self.k_age, age.as_millis() as f64)
            .with_evidence(self.k_limit, self.timeout.as_millis() as f64),
        );
    }
}

/// 일부러 패닉하는 감지기. 격리(R-18)를 눈으로 확인하기 위한 모듈이다.
///
/// 배포용 설정에 넣지 말 것. `after_ticks` 번째 tick에서 패닉하고, 그 뒤로는
/// 스케줄러가 이 모듈을 건너뛴다. 같은 그룹의 다른 모듈은 계속 돈다.
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `after_ticks` | `5` | 몇 번째 tick에서 패닉할지 |
#[derive(Debug, Default)]
pub struct PanicProbe {
    after: u64,
    ticks: u64,
}

impl Detector for PanicProbe {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        self.after = count_param(ctx.params().f64_or("after_ticks", 5.0)?);
        Ok(())
    }

    fn requires(&self) -> Vec<String> {
        Vec::new()
    }

    #[allow(
        clippy::panic,
        clippy::manual_assert,
        reason = "demonstrating panic isolation is this module's only purpose"
    )]
    fn tick(&mut self, _now: Instant, _bus: &SignalBus, _out: &mut dyn EventSink) {
        self.ticks = self.ticks.saturating_add(1);
        if self.ticks >= self.after {
            panic!("panic_probe: deliberate panic at tick {}", self.ticks);
        }
    }

    fn health(&self) -> Health {
        Health::Ok
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::{CommTimeout, MinDistance, Threshold};
    use rsm_core::event::{ModuleId, Severity};
    use rsm_core::intern::Names;
    use rsm_core::schedule::VecSink;
    use rsm_core::signal::{Signal, SignalBus, SignalId};
    use rsm_core::time::Instant;
    use rsm_core::traits::{Detector, ModuleCtx, Params};
    use std::collections::BTreeMap;

    fn params(pairs: &[(&str, serde_yaml_ng::Value)]) -> Params {
        let mut m = BTreeMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_owned(), v.clone());
        }
        Params::new("t".to_owned(), m)
    }

    fn num(v: f64) -> serde_yaml_ng::Value {
        serde_yaml_ng::Value::Number(v.into())
    }

    fn text(v: &str) -> serde_yaml_ng::Value {
        serde_yaml_ng::Value::String(v.to_owned())
    }

    fn configure<D: Detector>(d: &mut D, p: &Params) {
        let mut names = Names::new();
        let mut ctx = ModuleCtx::new(ModuleId(0), "t".to_owned(), p, &mut names);
        d.configure(&mut ctx).unwrap();
    }

    #[test]
    fn threshold_fires_only_above_limit() {
        let p = params(&[("signal", text("x")), ("limit", num(10.0))]);
        let mut d = Threshold::default();
        configure(&mut d, &p);

        let mut bus = SignalBus::with_capacity(1);
        let mut out = VecSink::new();
        bus.write(SignalId(0), Signal::Scalar(9.9), Instant::from_nanos(1));
        d.tick(Instant::from_nanos(1), &bus, &mut out);
        assert!(out.events.is_empty());

        bus.write(SignalId(0), Signal::Scalar(10.1), Instant::from_nanos(2));
        d.tick(Instant::from_nanos(2), &bus, &mut out);
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].severity, Severity::Warning);
        assert_eq!(out.events[0].evidence.len(), 2);
    }

    #[test]
    fn threshold_stays_silent_when_signal_never_written() {
        let p = params(&[("signal", text("x")), ("limit", num(10.0))]);
        let mut d = Threshold::default();
        configure(&mut d, &p);

        let bus = SignalBus::with_capacity(1);
        let mut out = VecSink::new();
        d.tick(Instant::from_nanos(1), &bus, &mut out);
        // 값이 없을 때 임계 감지기는 아무 주장도 하지 않는다. 침묵의 판정은
        // comm_timeout 의 몫이다.
        assert!(out.events.is_empty());
    }

    #[test]
    fn min_distance_escalates_to_critical() {
        let p = params(&[
            ("signal", text("d")),
            ("warn_m", num(1.0)),
            ("critical_m", num(0.5)),
        ]);
        let mut d = MinDistance::default();
        configure(&mut d, &p);

        let mut bus = SignalBus::with_capacity(1);
        let mut out = VecSink::new();
        for (v, expect) in [
            (1.5, None),
            (0.8, Some(Severity::Warning)),
            (0.2, Some(Severity::Critical)),
        ] {
            out.events.clear();
            bus.write(SignalId(0), Signal::Distance(v), Instant::from_nanos(1));
            d.tick(Instant::from_nanos(1), &bus, &mut out);
            assert_eq!(out.events.first().map(|e| e.severity), expect);
        }
    }

    #[test]
    fn min_distance_rejects_inverted_thresholds() {
        let p = params(&[
            ("signal", text("d")),
            ("warn_m", num(0.5)),
            ("critical_m", num(1.0)),
        ]);
        let mut d = MinDistance::default();
        let mut names = Names::new();
        let mut ctx = ModuleCtx::new(ModuleId(0), "t".to_owned(), &p, &mut names);
        assert!(d.configure(&mut ctx).is_err());
    }

    #[test]
    fn comm_timeout_fires_after_silence() {
        let p = params(&[("signal", text("x")), ("timeout_ms", num(100.0))]);
        let mut d = CommTimeout::default();
        configure(&mut d, &p);

        let mut bus = SignalBus::with_capacity(1);
        let mut out = VecSink::new();
        bus.write(SignalId(0), Signal::Scalar(1.0), Instant::from_nanos(0));

        d.tick(Instant::from_nanos(50_000_000), &bus, &mut out);
        assert!(out.events.is_empty());

        d.tick(Instant::from_nanos(150_000_000), &bus, &mut out);
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].severity, Severity::Critical);
    }

    #[test]
    fn comm_timeout_fires_when_signal_never_arrives() {
        let p = params(&[("signal", text("x")), ("timeout_ms", num(100.0))]);
        let mut d = CommTimeout::default();
        configure(&mut d, &p);

        let bus = SignalBus::with_capacity(1);
        let mut out = VecSink::new();
        d.tick(Instant::from_nanos(1), &bus, &mut out);
        // 한 번도 오지 않은 입력도 두절이다.
        assert_eq!(out.events.len(), 1);
    }
}
