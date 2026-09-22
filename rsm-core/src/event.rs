//! 위험 이벤트와 종합 상태 스키마 (R-04, R-06).
//!
//! 파이프라인을 흐르는 타입은 셋뿐이다. [`Signal`](crate::signal::Signal)이
//! Source에서 Detector로, [`HazardEvent`]가 Detector에서 Arbiter로,
//! [`SafetyState`]가 Arbiter에서 Sink로 간다.
//!
//! [`HazardEvent`]는 `Copy`이고 힙을 쓰지 않는다. 1 kHz tick 경로에서 할당이
//! 일어나면 안 되기 때문이다(R-08). 이름은 전부 [`crate::intern`]의 정수 ID다.

use crate::time::{Duration, Instant};

/// 모듈 식별자. 설정 로드 시 모듈 이름을 인터닝한 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId(pub u16);

impl ModuleId {
    /// 아직 설정되지 않은 ID. [`SignalId::UNSET`](crate::signal::SignalId::UNSET) 참고.
    pub const UNSET: Self = Self(u16::MAX);
}

impl Default for ModuleId {
    fn default() -> Self {
        Self::UNSET
    }
}

/// 위험 타입 식별자. `internal.joint.torque_limit` 같은 계층 이름을 인터닝한 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HazardTypeId(pub u16);

impl HazardTypeId {
    /// 아직 설정되지 않은 ID.
    pub const UNSET: Self = Self(u16::MAX);
}

impl Default for HazardTypeId {
    fn default() -> Self {
        Self::UNSET
    }
}

/// 근거(evidence) 항목의 키. `measured`, `limit` 같은 이름을 인터닝한 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceKey(pub u16);

impl EvidenceKey {
    /// 아직 설정되지 않은 ID.
    pub const UNSET: Self = Self(u16::MAX);
}

impl Default for EvidenceKey {
    fn default() -> Self {
        Self::UNSET
    }
}

/// 위험의 심각도. **순서형**이라 최댓값을 취하는 것이 곧 융합 정책이 된다(R-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Severity {
    /// 위험 없음.
    #[default]
    None,
    /// 알림 수준. 운영자가 알면 좋지만 행동이 필요하지는 않다.
    Advisory,
    /// 경고. 접근 중이거나 여유가 줄고 있다.
    Warning,
    /// 위험. 한계를 넘었거나 즉시 대응이 필요하다.
    Critical,
}

impl Severity {
    /// 출력용 이름.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Advisory => "ADVISORY",
            Self::Warning => "WARNING",
            Self::Critical => "CRITICAL",
        }
    }
}

/// 위험의 출처 분류. 모듈 이름의 최상위 네임스페이스와 일치한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// 로봇 자체 상태에서 온 위험 — 관절 한계, 온도, 배터리, 통신.
    Internal,
    /// 환경에서 온 위험 — 사람 근접, 장애물 거리.
    External,
    /// 학습 모델이 낸 위험. 확신도가 1.0 미만인 것이 보통이다.
    Ml,
}

impl Category {
    /// 출력용 이름.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
            Self::Ml => "ml",
        }
    }
}

/// 한 이벤트가 담을 수 있는 근거 항목의 최대 개수.
///
/// 고정 크기라 `HazardEvent`가 `Copy`로 남는다. 넘치는 근거는 버려지고
/// [`EvidenceSet::push`]가 `false`를 돌려준다.
pub const MAX_EVIDENCE: usize = 4;

/// 판정의 근거가 된 수치들. `(키, 값)` 쌍을 고정 개수만큼 담는다.
///
/// 평탄한 구조라 나중에 `#[repr(C)]` 미러를 만들기 쉽다(P2 R-19).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvidenceSet {
    len: u8,
    items: [(EvidenceKey, f64); MAX_EVIDENCE],
}

impl Default for EvidenceSet {
    fn default() -> Self {
        Self::new()
    }
}

impl EvidenceSet {
    /// 빈 근거 집합.
    pub const fn new() -> Self {
        Self {
            len: 0,
            items: [(EvidenceKey(0), 0.0); MAX_EVIDENCE],
        }
    }

    /// 근거를 하나 추가한다. 가득 차 있으면 아무것도 하지 않고 `false`.
    pub fn push(&mut self, key: EvidenceKey, value: f64) -> bool {
        let idx = usize::from(self.len);
        if idx >= MAX_EVIDENCE {
            return false;
        }
        self.items[idx] = (key, value);
        self.len += 1;
        true
    }

    /// 담긴 항목 수.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// 비어 있는지.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 담긴 `(키, 값)` 쌍을 순회한다.
    pub fn iter(&self) -> impl Iterator<Item = (EvidenceKey, f64)> + '_ {
        self.items.iter().take(self.len()).copied()
    }
}

/// Detector 한 번의 판정 결과. **TTL 동안만 유효한 주장**이다.
///
/// "위험 발생"과 "위험 해제"를 쌍으로 보내는 대신 TTL을 둔 이유는, 해제
/// 메시지가 유실되거나 모듈이 죽으면 위험 상태가 영원히 남기 때문이다.
/// 갱신이 끊기면 스스로 소멸하는 쪽이 안전하다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HazardEvent {
    /// 이 이벤트를 낸 모듈.
    pub source: ModuleId,
    /// 위험 타입. 같은 타입의 이벤트는 Arbiter에서 최신값으로 갱신된다.
    pub kind: HazardTypeId,
    /// 출처 분류.
    pub category: Category,
    /// 심각도.
    pub severity: Severity,
    /// 확신도 `0.0..=1.0`. 규칙 기반 모듈은 1.0, ML 모듈은 모델 출력.
    pub confidence: f32,
    /// 관측 시각. 주입된 시계에서 온 값이다.
    pub observed_at: Instant,
    /// 유효 기간. 보통 모듈 주기의 2~3배로 잡는다.
    pub ttl: Duration,
    /// 판정 근거 수치.
    pub evidence: EvidenceSet,
}

impl HazardEvent {
    /// 근거 없이 이벤트를 만든다. 근거는 [`with_evidence`](Self::with_evidence)로 붙인다.
    pub const fn new(
        source: ModuleId,
        kind: HazardTypeId,
        category: Category,
        severity: Severity,
        observed_at: Instant,
        ttl: Duration,
    ) -> Self {
        Self {
            source,
            kind,
            category,
            severity,
            confidence: 1.0,
            observed_at,
            ttl,
            evidence: EvidenceSet::new(),
        }
    }

    /// 확신도를 바꾼 사본을 만든다.
    #[must_use]
    pub const fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence;
        self
    }

    /// 근거를 하나 붙인 사본을 만든다. 가득 차 있으면 조용히 무시된다.
    #[must_use]
    pub fn with_evidence(mut self, key: EvidenceKey, value: f64) -> Self {
        self.evidence.push(key, value);
        self
    }

    /// 이 이벤트가 만료되는 시각.
    pub fn expires_at(&self) -> Instant {
        self.observed_at + self.ttl
    }

    /// `now` 시점에 만료되었는지.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at()
    }
}

/// 감시 기능 자체가 얼마나 믿을 만한지 (R-06).
///
/// 이 필드가 있는 이유는 감시 시스템의 최악 실패가 오경보가 아니라 **조용해지는
/// 것**이기 때문이다. 모듈이 죽거나 입력이 끊겨서 이벤트가 안 오는 것과, 실제로
/// 위험이 없어서 안 오는 것은 다르다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum MonitorAvailability {
    /// 모든 모듈이 정상 동작 중.
    #[default]
    Available,
    /// 일부 모듈이 고장·스테일. 나머지는 돌고 있다.
    Degraded,
    /// 감시가 사실상 멈췄다. 이 상태의 `level`은 신뢰할 수 없다.
    Unavailable,
}

impl MonitorAvailability {
    /// 출력용 이름.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Degraded => "Degraded",
            Self::Unavailable => "Unavailable",
        }
    }
}

/// Arbiter가 산출하는 종합 상태. 소비 시스템이 보는 최종 산출물.
///
/// Arbiter 스레드에서만 만들어지므로 `Vec`을 써도 핫패스 제약과 무관하다.
#[derive(Debug, Clone, PartialEq)]
pub struct SafetyState {
    /// 활성 이벤트 중 최고 심각도.
    pub level: Severity,
    /// 감시 가용성. `level`을 해석할 때 반드시 함께 봐야 한다.
    pub availability: MonitorAvailability,
    /// 아직 만료되지 않은 이벤트들.
    pub active: Vec<HazardEvent>,
    /// 이 상태를 산출한 시각.
    pub updated_at: Instant,
}

impl SafetyState {
    /// 위험이 없고 감시는 정상인 상태.
    pub fn quiet(now: Instant) -> Self {
        Self {
            level: Severity::None,
            availability: MonitorAvailability::Available,
            active: Vec::new(),
            updated_at: now,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::{
        Category, EvidenceKey, EvidenceSet, HazardEvent, HazardTypeId, MAX_EVIDENCE, ModuleId,
        Severity,
    };
    use crate::time::{Duration, Instant};

    fn sample(now: Instant, ttl: Duration) -> HazardEvent {
        HazardEvent::new(
            ModuleId(1),
            HazardTypeId(2),
            Category::Internal,
            Severity::Warning,
            now,
            ttl,
        )
    }

    #[test]
    fn severity_orders_from_none_to_critical() {
        assert!(Severity::None < Severity::Advisory);
        assert!(Severity::Advisory < Severity::Warning);
        assert!(Severity::Warning < Severity::Critical);
        let worst = [Severity::Advisory, Severity::Critical, Severity::None]
            .into_iter()
            .max();
        assert_eq!(worst, Some(Severity::Critical));
    }

    #[test]
    fn event_expires_exactly_at_ttl() {
        let now = Instant::from_nanos(1_000);
        let ev = sample(now, Duration::from_nanos(200));
        assert!(!ev.is_expired(Instant::from_nanos(1_199)));
        assert!(ev.is_expired(Instant::from_nanos(1_200)));
    }

    #[test]
    fn evidence_set_is_bounded_and_reports_overflow() {
        let mut ev = EvidenceSet::new();
        for v in [1.0_f64, 2.0, 3.0, 4.0].into_iter().take(MAX_EVIDENCE) {
            assert!(ev.push(EvidenceKey(0), v));
        }
        assert_eq!(ev.len(), MAX_EVIDENCE);
        assert!(!ev.push(EvidenceKey(1), 99.0), "returns false once full");
        assert_eq!(ev.len(), MAX_EVIDENCE);
    }

    #[test]
    fn evidence_iterates_only_written_items() {
        let mut set = EvidenceSet::new();
        set.push(EvidenceKey(3), 1.5);
        set.push(EvidenceKey(4), 2.5);
        let got: Vec<_> = set.iter().collect();
        assert_eq!(got, vec![(EvidenceKey(3), 1.5), (EvidenceKey(4), 2.5)]);
    }

    #[test]
    fn hazard_event_is_copy_and_heap_free() {
        let ev = sample(Instant::ZERO, Duration::from_millis(1));
        let copied = ev;
        assert_eq!(ev, copied);
    }
}
