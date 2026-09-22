//! 이벤트를 종합 상태로 융합하는 기본 정책 (R-05).
//!
//! 정책은 **max-severity + TTL** 이다. 활성 이벤트 중 최고 심각도를 종합 수준으로
//! 삼고, TTL이 지난 이벤트는 목록에서 빠진다. 같은 위험 타입의 이벤트는 최신값으로
//! 덮어쓴다 — 같은 위험이 계속 보고되는 동안 목록이 불어나지 않게.
//!
//! 이 정책이 한 줄로 끝나는 것은 [`Severity`](crate::event::Severity)가 순서형이기
//! 때문이다. 확신도를 반영하는 정책이나 규칙 기반 승격은 P1(R-13)에서 다른
//! [`ArbiterPolicy`] 구현으로 붙인다.

use std::collections::BTreeMap;

use crate::event::{HazardEvent, HazardTypeId, MonitorAvailability, SafetyState, Severity};
use crate::time::Instant;
use crate::traits::ArbiterPolicy;

/// 최고 심각도를 취하는 기본 정책.
#[derive(Debug, Default)]
pub struct MaxSeverity {
    latest: BTreeMap<HazardTypeId, HazardEvent>,
}

impl MaxSeverity {
    /// 빈 정책.
    pub fn new() -> Self {
        Self::default()
    }
}

impl ArbiterPolicy for MaxSeverity {
    fn ingest(&mut self, event: HazardEvent) {
        // 같은 타입은 최신값으로 갱신한다. 오래된 관측이 새 관측을 이기지 않도록
        // 관측 시각을 비교한다.
        match self.latest.get(&event.kind) {
            Some(prev) if prev.observed_at > event.observed_at => {}
            _ => {
                self.latest.insert(event.kind, event);
            }
        }
    }

    fn evaluate(&mut self, now: Instant, availability: MonitorAvailability) -> SafetyState {
        self.latest.retain(|_, ev| !ev.is_expired(now));

        let mut active: Vec<HazardEvent> = self.latest.values().copied().collect();
        active.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.kind.cmp(&b.kind)));

        let level = active
            .iter()
            .map(|e| e.severity)
            .max()
            .unwrap_or(Severity::None);

        SafetyState {
            level,
            availability,
            active,
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
    use super::MaxSeverity;
    use crate::event::{
        Category, HazardEvent, HazardTypeId, ModuleId, MonitorAvailability, Severity,
    };
    use crate::time::{Duration, Instant};
    use crate::traits::ArbiterPolicy;

    fn ev(kind: u16, sev: Severity, at: u64, ttl_ns: u64) -> HazardEvent {
        HazardEvent::new(
            ModuleId(0),
            HazardTypeId(kind),
            Category::Internal,
            sev,
            Instant::from_nanos(at),
            Duration::from_nanos(ttl_ns),
        )
    }

    #[test]
    fn level_is_max_of_active_events() {
        let mut p = MaxSeverity::new();
        p.ingest(ev(1, Severity::Advisory, 0, 1_000));
        p.ingest(ev(2, Severity::Critical, 0, 1_000));
        p.ingest(ev(3, Severity::Warning, 0, 1_000));

        let s = p.evaluate(Instant::from_nanos(10), MonitorAvailability::Available);
        assert_eq!(s.level, Severity::Critical);
        assert_eq!(s.active.len(), 3);
        assert_eq!(
            s.active[0].severity,
            Severity::Critical,
            "sorted by severity, descending"
        );
    }

    #[test]
    fn expired_events_drop_out_and_lower_the_level() {
        let mut p = MaxSeverity::new();
        p.ingest(ev(1, Severity::Critical, 0, 100));
        p.ingest(ev(2, Severity::Advisory, 0, 10_000));

        assert_eq!(
            p.evaluate(Instant::from_nanos(50), MonitorAvailability::Available)
                .level,
            Severity::Critical
        );
        let later = p.evaluate(Instant::from_nanos(200), MonitorAvailability::Available);
        assert_eq!(
            later.level,
            Severity::Advisory,
            "the expired CRITICAL is dropped"
        );
        assert_eq!(later.active.len(), 1);
    }

    #[test]
    fn same_kind_is_replaced_not_accumulated() {
        let mut p = MaxSeverity::new();
        p.ingest(ev(7, Severity::Warning, 0, 10_000));
        p.ingest(ev(7, Severity::Critical, 100, 10_000));
        let s = p.evaluate(Instant::from_nanos(150), MonitorAvailability::Available);
        assert_eq!(
            s.active.len(),
            1,
            "events of the same kind do not accumulate"
        );
        assert_eq!(s.level, Severity::Critical);
    }

    #[test]
    fn stale_observation_does_not_override_newer_one() {
        let mut p = MaxSeverity::new();
        p.ingest(ev(7, Severity::Critical, 500, 10_000));
        p.ingest(ev(7, Severity::None, 100, 10_000)); // 더 오래된 관측
        let s = p.evaluate(Instant::from_nanos(600), MonitorAvailability::Available);
        assert_eq!(s.level, Severity::Critical);
    }

    #[test]
    fn availability_passes_through_untouched() {
        let mut p = MaxSeverity::new();
        let s = p.evaluate(Instant::ZERO, MonitorAvailability::Degraded);
        assert_eq!(s.level, Severity::None);
        assert_eq!(s.availability, MonitorAvailability::Degraded);
    }
}
