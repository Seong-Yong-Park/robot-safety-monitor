//! 감시기 자신의 건강을 감시한다 (R-06).
//!
//! 이 파일이 있는 이유는 감시 시스템의 최악 실패가 오경보가 아니라 **조용해지는
//! 것**이기 때문이다. 모듈이 패닉하거나 입력이 끊기거나 큐가 넘쳐서 이벤트가 안
//! 오는 것과, 실제로 위험이 없어서 안 오는 것은 전혀 다른 상태다.
//!
//! TTL 만료([`HazardEvent::is_expired`](crate::event::HazardEvent::is_expired))와
//! **독립된 메커니즘**이라는 점이 중요하다. TTL만 있으면 감시기가 죽었을 때 위험
//! 이벤트가 조용히 사라져 `NONE` 으로 보인다.
//!
//! 세 가지 고장이 모두 [`MonitorAvailability`] 한 곳으로 모인다.
//!
//! | 고장 | 어디서 기록되나 |
//! |---|---|
//! | 모듈 패닉 | 그룹 스레드의 `catch_unwind` 경계 → [`HealthRegistry::record_fault`] |
//! | 그룹 정지·스테일 | [`HealthRegistry::scan`] 이 마지막 tick 시각을 보고 판정 |
//! | 큐 오버플로 | Detector가 이벤트를 밀어 넣을 때 → [`HealthRegistry::record_overflow`] |

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::event::{ModuleId, MonitorAvailability};
use crate::time::{Duration, Instant};
use crate::traits::Health;

const H_OK: u8 = 0;
const H_DEGRADED: u8 = 1;
const H_FAULTED: u8 = 2;

const fn health_to_u8(h: Health) -> u8 {
    match h {
        Health::Ok => H_OK,
        Health::Degraded => H_DEGRADED,
        Health::Faulted => H_FAULTED,
    }
}

const fn u8_to_health(v: u8) -> Health {
    match v {
        H_DEGRADED => Health::Degraded,
        H_FAULTED => Health::Faulted,
        _ => Health::Ok,
    }
}

const A_AVAILABLE: u8 = 0;
const A_DEGRADED: u8 = 1;
const A_UNAVAILABLE: u8 = 2;

const fn u8_to_availability(v: u8) -> MonitorAvailability {
    match v {
        A_DEGRADED => MonitorAvailability::Degraded,
        A_UNAVAILABLE => MonitorAvailability::Unavailable,
        _ => MonitorAvailability::Available,
    }
}

/// 모듈 하나의 건강 상태. 전부 원자 변수라 락 없이 갱신된다.
#[derive(Debug, Default)]
struct ModuleSlot {
    state: AtomicU8,
    last_tick_ns: AtomicU64,
    faults: AtomicU64,
}

/// 그룹 하나의 실행 통계. `fast` tick 경로에서 쓰이므로 원자 연산만 쓴다.
#[derive(Debug, Default)]
struct GroupSlot {
    last_tick_ns: AtomicU64,
    ticks: AtomicU64,
    overruns: AtomicU64,
    overflows: AtomicU64,
    max_tick_ns: AtomicU64,
    sum_tick_ns: AtomicU64,
}

/// 그룹 하나의 통계 스냅샷. 종료 시 요약 출력에 쓴다.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupStats {
    /// 지금까지 실행한 tick 수.
    pub ticks: u64,
    /// 주기를 넘긴 tick 수.
    pub overruns: u64,
    /// 큐가 가득 차 버려진 이벤트 수.
    pub overflows: u64,
    /// 가장 오래 걸린 tick(ns).
    pub max_tick_ns: u64,
    /// 평균 tick 실행 시간(ns).
    pub mean_tick_ns: u64,
}

/// 모듈·그룹의 건강을 모아 두는 공유 표.
///
/// 그룹 스레드가 쓰고 Supervisor 스레드가 읽는다. 전부 원자 변수라 뮤텍스가 없고,
/// 따라서 `fast` tick 경로에 상한 없는 대기가 생기지 않는다.
#[derive(Debug)]
pub struct HealthRegistry {
    modules: Vec<ModuleSlot>,
    groups: Vec<GroupSlot>,
    availability: AtomicU8,
}

impl HealthRegistry {
    /// 모듈 `module_count` 개, 그룹 `group_count` 개짜리 표를 만든다.
    pub fn new(module_count: usize, group_count: usize) -> Self {
        Self {
            modules: (0..module_count).map(|_| ModuleSlot::default()).collect(),
            groups: (0..group_count).map(|_| GroupSlot::default()).collect(),
            availability: AtomicU8::new(A_AVAILABLE),
        }
    }

    /// 모듈이 이번 tick 을 무사히 마쳤음을 기록한다.
    pub fn record_module_tick(&self, id: ModuleId, now: Instant) {
        if let Some(m) = self.modules.get(usize::from(id.0)) {
            m.last_tick_ns.store(now.as_nanos(), Ordering::Release);
        }
    }

    /// 모듈이 패닉했음을 기록한다. 고장 횟수가 누적된다.
    pub fn record_fault(&self, id: ModuleId) {
        if let Some(m) = self.modules.get(usize::from(id.0)) {
            m.faults.fetch_add(1, Ordering::Relaxed);
            m.state.store(H_FAULTED, Ordering::Release);
        }
    }

    /// 모듈이 스스로 보고한 상태를 반영한다.
    pub fn set_module_health(&self, id: ModuleId, h: Health) {
        if let Some(m) = self.modules.get(usize::from(id.0)) {
            m.state.store(health_to_u8(h), Ordering::Release);
        }
    }

    /// 모듈의 현재 상태.
    pub fn module_health(&self, id: ModuleId) -> Health {
        self.modules.get(usize::from(id.0)).map_or(Health::Ok, |m| {
            u8_to_health(m.state.load(Ordering::Acquire))
        })
    }

    /// 모듈의 누적 고장 횟수.
    pub fn module_faults(&self, id: ModuleId) -> u64 {
        self.modules
            .get(usize::from(id.0))
            .map_or(0, |m| m.faults.load(Ordering::Relaxed))
    }

    /// 그룹이 tick 을 마쳤음을 기록한다. `fast` 경로에서 불리므로 원자 연산만 한다.
    pub fn record_group_tick(&self, group: usize, at: Instant, elapsed: Duration, overran: bool) {
        let Some(g) = self.groups.get(group) else {
            return;
        };
        let ns = elapsed.as_nanos();
        g.last_tick_ns.store(at.as_nanos(), Ordering::Release);
        g.ticks.fetch_add(1, Ordering::Relaxed);
        g.sum_tick_ns.fetch_add(ns, Ordering::Relaxed);
        g.max_tick_ns.fetch_max(ns, Ordering::Relaxed);
        if overran {
            g.overruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 큐가 가득 차 이벤트를 버렸음을 기록한다.
    ///
    /// 조용히 버리지 않고 세는 이유는, 이벤트 유실이 곧 감시 실패이기 때문이다.
    /// [`scan`](Self::scan) 이 이 값을 보고 가용성을 낮춘다.
    pub fn record_overflow(&self, group: usize) {
        if let Some(g) = self.groups.get(group) {
            g.overflows.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 그룹 통계 스냅샷.
    pub fn group_stats(&self, group: usize) -> GroupStats {
        let Some(g) = self.groups.get(group) else {
            return GroupStats::default();
        };
        let ticks = g.ticks.load(Ordering::Relaxed);
        let sum = g.sum_tick_ns.load(Ordering::Relaxed);
        GroupStats {
            ticks,
            overruns: g.overruns.load(Ordering::Relaxed),
            overflows: g.overflows.load(Ordering::Relaxed),
            max_tick_ns: g.max_tick_ns.load(Ordering::Relaxed),
            mean_tick_ns: sum.checked_div(ticks).unwrap_or(0),
        }
    }

    /// 현재 가용성. Arbiter 스레드가 매번 읽는다.
    pub fn availability(&self) -> MonitorAvailability {
        u8_to_availability(self.availability.load(Ordering::Acquire))
    }

    /// 표 전체를 훑어 가용성을 다시 판정한다. Supervisor 스레드가 주기적으로 부른다.
    ///
    /// 판정 규칙은 보수적이다. 그룹이 멈췄으면 `Unavailable`, 모듈이 고장났거나
    /// 이벤트가 유실됐으면 `Degraded`.
    pub fn scan(&self, now: Instant, group_stale_after: Duration) -> MonitorAvailability {
        let mut worst = MonitorAvailability::Available;

        for g in &self.groups {
            let ticks = g.ticks.load(Ordering::Relaxed);
            if ticks == 0 {
                continue; // 아직 첫 tick 전이다 — 판정하지 않는다
            }
            let last = Instant::from_nanos(g.last_tick_ns.load(Ordering::Acquire));
            if now.saturating_duration_since(last) > group_stale_after {
                worst = MonitorAvailability::Unavailable;
            }
            if g.overflows.load(Ordering::Relaxed) > 0 && worst == MonitorAvailability::Available {
                worst = MonitorAvailability::Degraded;
            }
        }

        if worst == MonitorAvailability::Available {
            for m in &self.modules {
                if u8_to_health(m.state.load(Ordering::Acquire)) != Health::Ok {
                    worst = MonitorAvailability::Degraded;
                    break;
                }
            }
        }

        let code = match worst {
            MonitorAvailability::Available => A_AVAILABLE,
            MonitorAvailability::Degraded => A_DEGRADED,
            MonitorAvailability::Unavailable => A_UNAVAILABLE,
        };
        self.availability.store(code, Ordering::Release);
        worst
    }

    /// 고장 상태인 모듈들의 ID.
    pub fn faulted_modules(&self) -> Vec<ModuleId> {
        self.modules
            .iter()
            .enumerate()
            .filter(|(_, m)| u8_to_health(m.state.load(Ordering::Acquire)) == Health::Faulted)
            .filter_map(|(i, _)| u16::try_from(i).ok().map(ModuleId))
            .collect()
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::HealthRegistry;
    use crate::event::{ModuleId, MonitorAvailability};
    use crate::time::{Duration, Instant};

    const STALE: Duration = Duration::from_millis(100);

    #[test]
    fn healthy_pipeline_is_available() {
        let h = HealthRegistry::new(2, 1);
        h.record_group_tick(0, Instant::from_nanos(0), Duration::from_nanos(10), false);
        assert_eq!(
            h.scan(Instant::from_nanos(1_000), STALE),
            MonitorAvailability::Available
        );
    }

    #[test]
    fn module_panic_degrades_but_does_not_stop_the_pipeline() {
        let h = HealthRegistry::new(2, 1);
        h.record_group_tick(0, Instant::from_nanos(0), Duration::from_nanos(10), false);
        h.record_fault(ModuleId(1));

        assert_eq!(
            h.scan(Instant::from_nanos(1_000), STALE),
            MonitorAvailability::Degraded
        );
        assert_eq!(h.module_faults(ModuleId(1)), 1);
        assert_eq!(h.faulted_modules(), vec![ModuleId(1)]);
    }

    #[test]
    fn stopped_group_makes_monitoring_unavailable() {
        let h = HealthRegistry::new(1, 1);
        h.record_group_tick(0, Instant::from_nanos(0), Duration::from_nanos(10), false);
        // 100ms 를 넘겨 멈춰 있다
        let later = Instant::from_nanos(200_000_000);
        assert_eq!(h.scan(later, STALE), MonitorAvailability::Unavailable);
    }

    #[test]
    fn dropped_events_degrade_availability() {
        let h = HealthRegistry::new(1, 1);
        h.record_group_tick(0, Instant::from_nanos(0), Duration::from_nanos(10), false);
        h.record_overflow(0);
        assert_eq!(
            h.scan(Instant::from_nanos(1_000), STALE),
            MonitorAvailability::Degraded
        );
        assert_eq!(h.group_stats(0).overflows, 1);
    }

    #[test]
    fn before_first_tick_nothing_is_judged() {
        let h = HealthRegistry::new(1, 1);
        assert_eq!(
            h.scan(Instant::from_nanos(999_999_999), STALE),
            MonitorAvailability::Available
        );
    }

    #[test]
    fn stats_accumulate_mean_and_max() {
        let h = HealthRegistry::new(0, 1);
        h.record_group_tick(0, Instant::ZERO, Duration::from_nanos(10), false);
        h.record_group_tick(0, Instant::ZERO, Duration::from_nanos(30), true);
        let s = h.group_stats(0);
        assert_eq!(s.ticks, 2);
        assert_eq!(s.overruns, 1);
        assert_eq!(s.max_tick_ns, 30);
        assert_eq!(s.mean_tick_ns, 20);
    }
}
