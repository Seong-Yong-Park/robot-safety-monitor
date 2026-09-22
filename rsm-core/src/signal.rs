//! Source가 내고 Detector가 읽는 신호 (PRD §10).
//!
//! 고정된 enum이라 variant를 늘리면 `match`의 철저성 검사 덕분에 처리하지 않은
//! 곳이 전부 빌드 에러로 드러난다. 확장이 필요하면 [`Signal::Custom`]을 쓴다.
//!
//! [`SignalBus`]는 그룹 스레드 안에서만 쓰인다. 한 tick에서 Source를 먼저 돌리고
//! Detector를 돌리므로 같은 스레드 안의 순차 접근이고, 락이 필요 없다.

use crate::time::Instant;

/// 신호 식별자. 설정 로드 시 신호 이름을 인터닝한 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignalId(pub u16);

impl SignalId {
    /// 아직 설정되지 않은 ID.
    ///
    /// 인터너는 `0..u16::MAX` 범위만 내주므로 이 값은 절대 실제 신호를 가리키지
    /// 않는다. 이 ID로 쓰면 버스가 조용히 무시하고, 읽으면 `None`이 된다.
    /// 즉 `configure`를 거치지 않은 모듈은 아무 일도 하지 않는다.
    pub const UNSET: Self = Self(u16::MAX);
}

impl Default for SignalId {
    fn default() -> Self {
        Self::UNSET
    }
}

/// 벡터 신호가 담을 수 있는 최대 원소 수. 고정 크기라 힙을 쓰지 않는다.
pub const MAX_VECTOR: usize = 32;

/// 고정 길이 실수 벡터. 관절 상태처럼 원소 수가 여럿인 신호에 쓴다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedVec {
    len: u8,
    data: [f64; MAX_VECTOR],
}

impl Default for FixedVec {
    fn default() -> Self {
        Self::new()
    }
}

impl FixedVec {
    /// 빈 벡터.
    pub const fn new() -> Self {
        Self {
            len: 0,
            data: [0.0; MAX_VECTOR],
        }
    }

    /// 슬라이스에서 만든다. `MAX_VECTOR`를 넘는 뒷부분은 잘린다.
    pub fn from_slice(values: &[f64]) -> Self {
        let mut v = Self::new();
        for &x in values.iter().take(MAX_VECTOR) {
            v.push(x);
        }
        v
    }

    /// 원소를 하나 추가한다. 가득 차 있으면 `false`.
    pub fn push(&mut self, value: f64) -> bool {
        let idx = usize::from(self.len);
        if idx >= MAX_VECTOR {
            return false;
        }
        self.data[idx] = value;
        self.len += 1;
        true
    }

    /// 원소 수.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// 비어 있는지.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 담긴 원소들.
    pub fn as_slice(&self) -> &[f64] {
        &self.data[..self.len()]
    }
}

/// 신호의 종류. 설정 검증에서 Source의 출력과 Detector의 입력 타입을 맞출 때 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// 참/거짓.
    Bool,
    /// 단일 실수 — 전압, 온도, 토크 등.
    Scalar,
    /// 거리(m). 스칼라와 단위가 다르므로 타입을 나눈다.
    Distance,
    /// 실수 벡터 — 관절 위치·속도·토크 묶음.
    Vector,
}

impl SignalKind {
    /// 설정 파일에 적는 이름.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Scalar => "scalar",
            Self::Distance => "distance",
            Self::Vector => "vector",
        }
    }

    /// 설정 파일의 이름에서 종류를 읽는다.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bool" => Some(Self::Bool),
            "scalar" => Some(Self::Scalar),
            "distance" => Some(Self::Distance),
            "vector" => Some(Self::Vector),
            _ => None,
        }
    }
}

/// 한 시점의 관측값.
///
/// `Vector` variant 가 다른 것들보다 훨씬 크다(고정 배열 264바이트 대 8바이트).
/// 이를 `Box` 로 줄이면 힙을 쓰게 되는데, 신호 저장소는 기동 시 한 번만 할당되고
/// tick 경로에서는 읽기만 하므로 크기보다 **할당 없음**을 택했다(R-08).
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "deliberate: keeps the fixed-size vector off the heap (R-08)"
)]
pub enum Signal {
    /// 참/거짓.
    Bool(bool),
    /// 단일 실수.
    Scalar(f64),
    /// 거리(m).
    Distance(f64),
    /// 실수 벡터.
    Vector(FixedVec),
}

impl Signal {
    /// 이 값의 종류.
    pub const fn kind(&self) -> SignalKind {
        match self {
            Self::Bool(_) => SignalKind::Bool,
            Self::Scalar(_) => SignalKind::Scalar,
            Self::Distance(_) => SignalKind::Distance,
            Self::Vector(_) => SignalKind::Vector,
        }
    }

    /// 스칼라나 거리면 그 실수, 아니면 `None`.
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Scalar(v) | Self::Distance(v) => Some(*v),
            _ => None,
        }
    }

    /// 벡터면 그 슬라이스, 아니면 `None`.
    pub fn as_slice(&self) -> Option<&[f64]> {
        match self {
            Self::Vector(v) => Some(v.as_slice()),
            _ => None,
        }
    }
}

/// 버스의 한 칸. 값과 함께 **언제 쓰였는지**를 남긴다.
///
/// 갱신 시각이 있어야 Supervisor가 "입력이 끊겼다"를 판정할 수 있다(R-06).
#[derive(Debug, Clone, Copy)]
pub struct SignalSlot {
    /// 마지막으로 쓰인 값. 한 번도 안 쓰였으면 `None`.
    pub value: Option<Signal>,
    /// 마지막으로 쓰인 시각.
    pub updated_at: Instant,
    /// 갱신 횟수. 값이 같아도 갱신되었는지 구분할 때 쓴다.
    pub seq: u64,
}

impl SignalSlot {
    const fn empty() -> Self {
        Self {
            value: None,
            updated_at: Instant::ZERO,
            seq: 0,
        }
    }

    /// `now` 기준으로 `max_age`보다 오래되었는지. 한 번도 안 쓰였으면 스테일로 본다.
    pub fn is_stale(&self, now: Instant, max_age: crate::time::Duration) -> bool {
        self.value.is_none() || now.saturating_duration_since(self.updated_at) > max_age
    }
}

/// 그룹 하나가 쓰는 신호 저장소. 초기화 때 크기가 정해지고 이후 늘지 않는다.
#[derive(Debug, Clone)]
pub struct SignalBus {
    slots: Vec<SignalSlot>,
}

impl SignalBus {
    /// 신호 `count`개를 담을 버스를 만든다. 할당은 여기서 한 번만 일어난다.
    pub fn with_capacity(count: usize) -> Self {
        Self {
            slots: vec![SignalSlot::empty(); count],
        }
    }

    /// 담을 수 있는 신호 수.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 비어 있는지.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 값을 쓴다. 범위를 벗어난 ID는 조용히 무시된다(설정 검증에서 이미 걸러진다).
    pub fn write(&mut self, id: SignalId, value: Signal, now: Instant) {
        if let Some(slot) = self.slots.get_mut(usize::from(id.0)) {
            slot.value = Some(value);
            slot.updated_at = now;
            slot.seq = slot.seq.wrapping_add(1);
        }
    }

    /// 칸을 읽는다.
    pub fn slot(&self, id: SignalId) -> Option<&SignalSlot> {
        self.slots.get(usize::from(id.0))
    }

    /// 값을 읽는다. 한 번도 안 쓰였으면 `None`.
    pub fn read(&self, id: SignalId) -> Option<Signal> {
        self.slot(id).and_then(|s| s.value)
    }

    /// 스칼라나 거리 값을 바로 읽는다.
    pub fn read_f64(&self, id: SignalId) -> Option<f64> {
        self.read(id).and_then(|s| s.as_f64())
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::{FixedVec, MAX_VECTOR, Signal, SignalBus, SignalId, SignalKind};
    use crate::time::{Duration, Instant};

    #[test]
    fn bus_starts_empty_and_records_write_time() {
        let mut bus = SignalBus::with_capacity(4);
        let id = SignalId(2);
        assert_eq!(bus.read(id), None);

        let t = Instant::from_nanos(500);
        bus.write(id, Signal::Scalar(1.25), t);
        assert_eq!(bus.read_f64(id), Some(1.25));

        let slot = bus.slot(id).copied().unwrap_or_else(|| {
            panic!("slot just written is missing");
        });
        assert_eq!(slot.updated_at, t);
        assert_eq!(slot.seq, 1);
    }

    #[test]
    fn never_written_slot_counts_as_stale() {
        let bus = SignalBus::with_capacity(1);
        let slot = bus.slot(SignalId(0)).copied().unwrap_or_else(|| {
            panic!("slot is missing");
        });
        assert!(slot.is_stale(Instant::ZERO, Duration::from_millis(100)));
    }

    #[test]
    fn stale_after_threshold() {
        let mut bus = SignalBus::with_capacity(1);
        let id = SignalId(0);
        bus.write(id, Signal::Bool(true), Instant::from_nanos(0));
        let slot = bus
            .slot(id)
            .copied()
            .unwrap_or_else(|| panic!("slot is missing"));

        let limit = Duration::from_millis(100);
        assert!(!slot.is_stale(Instant::from_nanos(100_000_000), limit));
        assert!(slot.is_stale(Instant::from_nanos(100_000_001), limit));
    }

    #[test]
    fn out_of_range_write_is_ignored() {
        let mut bus = SignalBus::with_capacity(1);
        bus.write(SignalId(99), Signal::Scalar(1.0), Instant::ZERO);
        assert_eq!(bus.read(SignalId(99)), None);
    }

    #[test]
    fn fixed_vec_truncates_beyond_capacity() {
        let long = [1.0_f64; MAX_VECTOR + 5];
        let v = FixedVec::from_slice(&long);
        assert_eq!(v.len(), MAX_VECTOR);
    }

    #[test]
    fn signal_kind_round_trips_through_config_name() {
        for k in [
            SignalKind::Bool,
            SignalKind::Scalar,
            SignalKind::Distance,
            SignalKind::Vector,
        ] {
            assert_eq!(SignalKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(SignalKind::parse("no_such_kind"), None);
    }
}
