//! # rsm-modules
//!
//! 레퍼런스 감지 모듈 세트 (PRD R-09).
//!
//! 각 모듈은 [`rsm_core`]의 [`Source`](rsm_core::traits::Source) 또는
//! [`Detector`](rsm_core::traits::Detector) 트레이트만 구현한다. 코어는 이 크레이트를
//! 알지 못하고, 이 크레이트는 스케줄러·큐·출력 형식을 알지 못한다. 새 모듈을 더할 때
//! 손대는 공용 지점은 [`register_all`] 한 줄뿐이다 (PRD G2 / TECH_STACK T4).
//!
//! ## 제공 모듈
//!
//! | `type:` | 역할 |
//! |---|---|
//! | `ramp` | 톱니파 신호를 만든다. 데모·시험용 Source |
//! | `constant` | 고정값 신호를 만든다 |
//! | `stalling` | N tick 뒤 갱신을 멈춘다. 통신 두절 재현용 |
//! | `threshold` | 신호가 한계를 넘으면 이벤트 |
//! | `min_distance` | 거리 신호가 가까워지면 2단계 이벤트 |
//! | `comm_timeout` | 신호 갱신이 끊기면 이벤트 |
//! | `panic_probe` | 일부러 패닉한다. 격리(R-18) 시연용 |

pub mod detectors;
pub mod sources;

use rsm_core::registry::Registry;
use rsm_core::time::Duration;

/// 이 크레이트가 제공하는 모든 모듈을 레지스트리에 등록한다.
///
/// 새 모듈을 추가할 때 손대는 유일한 공용 지점이다. `rsm-core`는 수정하지 않는다.
pub fn register_all(reg: &mut Registry) {
    reg.register_source("ramp", || Box::new(sources::Ramp::default()))
        .register_source("constant", || Box::new(sources::Constant::default()))
        .register_source("stalling", || Box::new(sources::Stalling::default()))
        .register_detector("threshold", || Box::new(detectors::Threshold::default()))
        .register_detector("min_distance", || {
            Box::new(detectors::MinDistance::default())
        })
        .register_detector("comm_timeout", || {
            Box::new(detectors::CommTimeout::default())
        })
        .register_detector("panic_probe", || Box::new(detectors::PanicProbe::default()));
}

/// 밀리초 파라미터를 [`Duration`]으로 바꾼다.
///
/// 설정 파일의 숫자는 `f64`로 들어온다. 음수·NaN·터무니없이 큰 값을 그대로
/// `u64`로 캐스팅하면 조용히 엉뚱한 값이 되므로 여기서 한 번 걸러 낸다.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped finite, non-negative and bounded just above the cast"
)]
pub(crate) fn millis_to_duration(ms: f64) -> Duration {
    let clamped = if ms.is_finite() {
        ms.clamp(0.0, 3_600_000.0)
    } else {
        0.0
    };
    Duration::from_nanos((clamped * 1_000_000.0) as u64)
}

/// 개수 파라미터를 `u64`로 바꾼다. 위와 같은 이유로 한 번 조인다.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped finite, non-negative and bounded just above the cast"
)]
pub(crate) fn count_param(v: f64) -> u64 {
    let clamped = if v.is_finite() {
        v.clamp(0.0, 1e12)
    } else {
        0.0
    };
    clamped as u64
}
