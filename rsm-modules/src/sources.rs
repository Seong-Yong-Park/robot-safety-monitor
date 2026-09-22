//! 시험·데모용 Source 모듈.
//!
//! 실제 로봇에서는 이 자리에 ROS 2 구독자나 CAN 프레임 파서가 들어간다.
//! Phase 0에서는 "설정만 바꿔 조합이 달라진다"를 보이는 것이 목적이라,
//! 외부 의존이 없는 신호 생성기를 쓴다.

use rsm_core::error::ConfigError;
use rsm_core::signal::{Signal, SignalBus, SignalId};
use rsm_core::time::Instant;
use rsm_core::traits::{Health, ModuleCtx, Source};

use crate::count_param;

/// 톱니파 Source. `min`에서 `max`까지 `step`씩 올리고 다시 `min`으로 돌아간다.
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `signal` | (필수) | 내보낼 신호 이름 |
/// | `min` | `0.0` | 시작값 |
/// | `max` | `100.0` | 되돌아가는 상한 |
/// | `step` | `1.0` | tick 당 증가량 |
#[derive(Debug, Default)]
pub struct Ramp {
    signal_name: String,
    signal: SignalId,
    min: f64,
    max: f64,
    step: f64,
    value: f64,
}

impl Source for Ramp {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        self.signal_name = ctx.params().str("signal")?.to_owned();
        self.min = ctx.params().f64_or("min", 0.0)?;
        self.max = ctx.params().f64_or("max", 100.0)?;
        self.step = ctx.params().f64_or("step", 1.0)?;
        if self.max <= self.min || !self.max.is_finite() || !self.min.is_finite() {
            return Err(ConfigError::BadParam {
                module: ctx.module_name().to_owned(),
                key: "max".to_owned(),
                expected: "a finite value greater than `min`",
            });
        }
        self.value = self.min;
        self.signal = ctx.signal(&self.signal_name)?;
        Ok(())
    }

    fn declares(&self) -> Vec<String> {
        vec![self.signal_name.clone()]
    }

    fn poll(&mut self, now: Instant, bus: &mut SignalBus) {
        bus.write(self.signal, Signal::Scalar(self.value), now);
        self.value += self.step;
        if self.value > self.max {
            self.value = self.min;
        }
    }
}

/// 고정값 Source. 다른 모듈의 기준선을 만들 때 쓴다.
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `signal` | (필수) | 내보낼 신호 이름 |
/// | `value` | `0.0` | 매 tick 쓰는 값 |
#[derive(Debug, Default)]
pub struct Constant {
    signal_name: String,
    signal: SignalId,
    value: f64,
}

impl Source for Constant {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        self.signal_name = ctx.params().str("signal")?.to_owned();
        self.value = ctx.params().f64_or("value", 0.0)?;
        self.signal = ctx.signal(&self.signal_name)?;
        Ok(())
    }

    fn declares(&self) -> Vec<String> {
        vec![self.signal_name.clone()]
    }

    fn poll(&mut self, now: Instant, bus: &mut SignalBus) {
        bus.write(self.signal, Signal::Scalar(self.value), now);
    }
}

/// `stall_after` tick 뒤에 갱신을 멈추는 Source. 통신 두절을 재현한다.
///
/// 멈춘 뒤에도 모듈 자체는 살아 있고 패닉하지도 않는다. "입력이 조용한 것"과
/// "위험이 없는 것"이 다르다는 걸 보이는 것이 이 모듈의 쓸모다 (R-16).
///
/// | 파라미터 | 기본값 | 뜻 |
/// |---|---|---|
/// | `signal` | (필수) | 내보낼 신호 이름 |
/// | `value` | `0.0` | 멈추기 전까지 쓰는 값 |
/// | `stall_after` | `50` | 몇 tick 뒤 멈출지 |
#[derive(Debug, Default)]
pub struct Stalling {
    signal_name: String,
    signal: SignalId,
    value: f64,
    stall_after: u64,
    ticks: u64,
}

impl Source for Stalling {
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
        self.signal_name = ctx.params().str("signal")?.to_owned();
        self.value = ctx.params().f64_or("value", 0.0)?;
        self.stall_after = count_param(ctx.params().f64_or("stall_after", 50.0)?);
        self.signal = ctx.signal(&self.signal_name)?;
        Ok(())
    }

    fn declares(&self) -> Vec<String> {
        vec![self.signal_name.clone()]
    }

    fn poll(&mut self, now: Instant, bus: &mut SignalBus) {
        self.ticks = self.ticks.saturating_add(1);
        if self.ticks <= self.stall_after {
            bus.write(self.signal, Signal::Scalar(self.value), now);
        }
    }

    fn health(&self) -> Health {
        // 스스로는 고장을 모른다. 침묵을 알아채는 것은 `comm_timeout` Detector와
        // Supervisor의 몫이다.
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
    use super::{Ramp, Stalling};
    use rsm_core::event::ModuleId;
    use rsm_core::intern::Names;
    use rsm_core::signal::{SignalBus, SignalId};
    use rsm_core::time::Instant;
    use rsm_core::traits::{ModuleCtx, Params, Source};
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

    #[test]
    fn ramp_wraps_at_max() {
        let p = params(&[
            ("signal", text("x")),
            ("min", num(0.0)),
            ("max", num(2.0)),
            ("step", num(1.0)),
        ]);
        let mut names = Names::new();
        let mut r = Ramp::default();
        {
            let mut ctx = ModuleCtx::new(ModuleId(0), "t".to_owned(), &p, &mut names);
            r.configure(&mut ctx).unwrap();
        }
        let mut bus = SignalBus::with_capacity(1);
        let mut seen = Vec::new();
        for i in 0..5u64 {
            r.poll(Instant::from_nanos(i), &mut bus);
            seen.push(bus.read_f64(SignalId(0)).unwrap());
        }
        assert_eq!(seen, vec![0.0, 1.0, 2.0, 0.0, 1.0]);
    }

    #[test]
    fn stalling_stops_writing() {
        let p = params(&[
            ("signal", text("x")),
            ("value", num(7.0)),
            ("stall_after", num(2.0)),
        ]);
        let mut names = Names::new();
        let mut s = Stalling::default();
        {
            let mut ctx = ModuleCtx::new(ModuleId(0), "t".to_owned(), &p, &mut names);
            s.configure(&mut ctx).unwrap();
        }
        let mut bus = SignalBus::with_capacity(1);
        for i in 1..=4u64 {
            s.poll(Instant::from_nanos(i), &mut bus);
        }
        // 마지막 갱신 시각이 2번째 tick에 멈춰 있다.
        let slot = bus.slot(SignalId(0)).copied().unwrap();
        assert_eq!(slot.updated_at, Instant::from_nanos(2));
    }
}
