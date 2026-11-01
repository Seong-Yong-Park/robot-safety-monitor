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
//! 네 가지 고장이 모두 [`MonitorAvailability`] 한 곳으로 모인다.
//!
//! | 고장 | 어디서 기록되나 |
//! |---|---|
//! | 모듈 패닉 | 그룹 스레드의 `catch_unwind` 경계 → [`HealthRegistry::record_fault`] |
//! | 그룹 정지·스테일 | [`HealthRegistry::scan`] 이 마지막 tick 시각을 보고 판정 |
//! | 큐 오버플로 | Detector가 이벤트를 밀어 넣을 때 → [`HealthRegistry::record_overflow`] |
//! | 데드라인 미스 | 그룹 스레드의 tick 끝 → [`HealthRegistry::record_group_tick`] |

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
    first_tick_ns: AtomicU64,
    last_tick_ns: AtomicU64,
    ticks: AtomicU64,
    misses: AtomicU64,
    /// 직전 scan 때 본 `misses`. Supervisor 스레드만 쓴다 (PRD D25).
    misses_seen: AtomicU64,
    overflows: AtomicU64,
    max_work_ns: AtomicU64,
    sum_work_ns: AtomicU64,
    max_late_ns: AtomicU64,
    sum_late_ns: AtomicU64,
}

/// 그룹 하나의 통계 스냅샷. 종료 시 요약 출력에 쓴다.
///
/// 시간 지표가 셋으로 나뉜다. 작업 시간만 보면 주기를 절반밖에 못 지켜도
/// 문제가 안 보이기 때문이다(Phase 0 실측).
///
/// | 지표 | 뜻 |
/// |---|---|
/// | 작업 시간 (work) | `ended − started` — 모듈들이 실제로 쓴 시간 |
/// | 시작 지연 (late) | `started − release` — 예정 시각보다 늦게 깬 정도 (지터) |
/// | 데드라인 미스 (miss) | `ended > release + period` — 다음 예정 시각까지 못 끝냄 |
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupStats {
    /// 지금까지 실행한 tick 수.
    pub ticks: u64,
    /// 데드라인을 놓친 tick 수.
    pub misses: u64,
    /// 큐가 가득 차 버려진 이벤트 수.
    pub overflows: u64,
    /// 가장 긴 작업 시간(ns).
    pub max_work_ns: u64,
    /// 평균 작업 시간(ns).
    pub mean_work_ns: u64,
    /// 가장 큰 시작 지연(ns).
    pub max_late_ns: u64,
    /// 평균 시작 지연(ns).
    pub mean_late_ns: u64,
    /// 첫 tick 과 마지막 tick 의 시작 시각 차(ns). 실효 주파수 계산에 쓴다.
    pub span_ns: u64,
}

impl GroupStats {
    /// 실제로 돈 주파수(mHz). 첫 tick 과 마지막 tick 사이의 간격 수로 센다.
    ///
    /// 설정한 주파수와 다르면 tick 을 건너뛰었다는 뜻이다. 부동소수점 대신
    /// 밀리헤르츠 정수로 돌려준다. tick 이 둘 미만이면 0.
    pub fn effective_millihertz(&self) -> u64 {
        let intervals = u128::from(self.ticks.saturating_sub(1));
        let mhz = (intervals * 1_000_000_000_000)
            .checked_div(u128::from(self.span_ns))
            .unwrap_or(0);
        u64::try_from(mhz).unwrap_or(u64::MAX)
    }
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
    ///
    /// `release`는 이 tick 이 시작하기로 예정된 시각, `started`·`ended`는 실제로
    /// 시작하고 끝난 시각이다. 지표의 정의는 [`GroupStats`]에 있고, 판정은 여기
    /// 한 곳에서만 한다.
    pub fn record_group_tick(
        &self,
        group: usize,
        release: Instant,
        started: Instant,
        ended: Instant,
        period: Duration,
    ) {
        let Some(g) = self.groups.get(group) else {
            return;
        };
        let work = (ended - started).as_nanos();
        let late = (started - release).as_nanos();

        // 쓰는 스레드는 그룹 스레드 하나뿐이라, 이전 값이 0 이면 첫 tick 이다.
        if g.ticks.load(Ordering::Relaxed) == 0 {
            g.first_tick_ns.store(started.as_nanos(), Ordering::Relaxed);
        }
        g.last_tick_ns.store(started.as_nanos(), Ordering::Release);
        g.ticks.fetch_add(1, Ordering::Relaxed);
        g.sum_work_ns.fetch_add(work, Ordering::Relaxed);
        g.max_work_ns.fetch_max(work, Ordering::Relaxed);
        g.sum_late_ns.fetch_add(late, Ordering::Relaxed);
        g.max_late_ns.fetch_max(late, Ordering::Relaxed);
        if ended > release + period {
            g.misses.fetch_add(1, Ordering::Relaxed);
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
        let first = Instant::from_nanos(g.first_tick_ns.load(Ordering::Relaxed));
        let last = Instant::from_nanos(g.last_tick_ns.load(Ordering::Acquire));
        GroupStats {
            ticks,
            misses: g.misses.load(Ordering::Relaxed),
            overflows: g.overflows.load(Ordering::Relaxed),
            max_work_ns: g.max_work_ns.load(Ordering::Relaxed),
            mean_work_ns: g
                .sum_work_ns
                .load(Ordering::Relaxed)
                .checked_div(ticks)
                .unwrap_or(0),
            max_late_ns: g.max_late_ns.load(Ordering::Relaxed),
            mean_late_ns: g
                .sum_late_ns
                .load(Ordering::Relaxed)
                .checked_div(ticks)
                .unwrap_or(0),
            span_ns: (last - first).as_nanos(),
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
    ///
    /// 데드라인 미스만은 **구간 판정**이다 (PRD D25). 직전 scan 이후 새 미스가 있으면
    /// 이번 scan 은 `Degraded`, 없으면 그 이유로는 낮추지 않는다. 래치된 고장
    /// 플래그가 아니라 워치독 창과 같다. 다른 조건과 달리 회복하는 이유는, 비실시간
    /// 환경에서는 미스가 가끔 나므로 sticky 로 두면 늘 `Degraded`가 되기 때문이다.
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
            // 다른 그룹 때문에 이미 나빠졌어도 기준점은 매번 옮긴다. 그래야 다음 구간이
            // 이번 미스를 다시 세지 않는다.
            let misses = g.misses.load(Ordering::Relaxed);
            let missed_since_last_scan = g.misses_seen.swap(misses, Ordering::Relaxed) < misses;
            if (missed_since_last_scan || g.overflows.load(Ordering::Relaxed) > 0)
                && worst == MonitorAvailability::Available
            {
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
    const PERIOD: Duration = Duration::from_micros(1_000);

    /// 시각 0 에 예정대로 시작해 10 ns 만에 끝난 tick 하나.
    fn ok_tick(h: &HealthRegistry) {
        h.record_group_tick(
            0,
            Instant::ZERO,
            Instant::ZERO,
            Instant::from_nanos(10),
            PERIOD,
        );
    }

    /// 예정 시각 `release_us`, 실제 시작 `started_us`, 작업 시간 `work_us` (모두 µs).
    fn tick_us(h: &HealthRegistry, release_us: u64, started_us: u64, work_us: u64) {
        let us = |v: u64| Instant::from_nanos(v * 1_000);
        h.record_group_tick(
            0,
            us(release_us),
            us(started_us),
            us(started_us + work_us),
            PERIOD,
        );
    }

    #[test]
    fn healthy_pipeline_is_available() {
        let h = HealthRegistry::new(2, 1);
        ok_tick(&h);
        assert_eq!(
            h.scan(Instant::from_nanos(1_000), STALE),
            MonitorAvailability::Available
        );
    }

    #[test]
    fn module_panic_degrades_but_does_not_stop_the_pipeline() {
        let h = HealthRegistry::new(2, 1);
        ok_tick(&h);
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
        ok_tick(&h);
        // 100ms 를 넘겨 멈춰 있다
        let later = Instant::from_nanos(200_000_000);
        assert_eq!(h.scan(later, STALE), MonitorAvailability::Unavailable);
    }

    #[test]
    fn dropped_events_degrade_availability() {
        let h = HealthRegistry::new(1, 1);
        ok_tick(&h);
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
    fn stats_separate_work_lateness_and_misses() {
        let h = HealthRegistry::new(0, 1);
        tick_us(&h, 0, 0, 10); //        예정대로 시작
        tick_us(&h, 1_000, 1_060, 30); // 60 µs 늦게 깼지만 데드라인(2_000) 안에 끝남
        let s = h.group_stats(0);
        assert_eq!(s.ticks, 2);
        assert_eq!(s.misses, 0);
        assert_eq!(s.max_work_ns, 30_000);
        assert_eq!(s.mean_work_ns, 20_000);
        assert_eq!(s.max_late_ns, 60_000);
        assert_eq!(s.mean_late_ns, 30_000);
    }

    #[test]
    fn short_work_still_misses_when_it_starts_too_late() {
        // 작업은 10 µs 로 짧다. 예전 판정(작업 시간 > 주기)은 이것을 못 봤다.
        let h = HealthRegistry::new(0, 1);
        tick_us(&h, 1_000, 1_995, 10); // 끝난 시각 2_005 > 데드라인 2_000
        assert_eq!(h.group_stats(0).misses, 1);
    }

    #[test]
    fn ending_exactly_on_the_deadline_is_not_a_miss() {
        let h = HealthRegistry::new(0, 1);
        tick_us(&h, 0, 0, 1_000);
        assert_eq!(h.group_stats(0).misses, 0);
    }

    #[test]
    fn effective_frequency_counts_intervals_between_first_and_last_tick() {
        let h = HealthRegistry::new(0, 1);
        for k in 0..=1_000 {
            tick_us(&h, k * 1_000, k * 1_000, 5);
        }
        // 1001 tick = 간격 1000 개, 1 초 → 1000 Hz
        assert_eq!(h.group_stats(0).effective_millihertz(), 1_000_000);
    }

    #[test]
    fn effective_frequency_reveals_skipped_ticks() {
        let h = HealthRegistry::new(0, 1);
        for k in 0..=500 {
            tick_us(&h, k * 2_000, k * 2_000, 5); // 1 kHz 설정인데 두 칸마다 한 번
        }
        assert_eq!(h.group_stats(0).effective_millihertz(), 500_000);
    }

    #[test]
    fn effective_frequency_needs_two_ticks() {
        let h = HealthRegistry::new(0, 1);
        assert_eq!(h.group_stats(0).effective_millihertz(), 0);
        ok_tick(&h);
        assert_eq!(h.group_stats(0).effective_millihertz(), 0);
    }

    // ── 데드라인 미스 → availability (PRD D25) ─────────────────────────────────

    /// 그룹 0 의 마지막 tick 근처 시각. 스테일 판정에 걸리지 않는다.
    const NEAR: Instant = Instant::from_nanos(3_000_000);

    #[test]
    fn miss_degrades_only_the_scan_window_it_happened_in() {
        let h = HealthRegistry::new(0, 1);
        tick_us(&h, 0, 0, 10);
        assert_eq!(h.scan(NEAR, STALE), MonitorAvailability::Available);

        tick_us(&h, 1_000, 1_995, 10); // 데드라인 2_000 을 넘겨 끝남
        assert_eq!(h.scan(NEAR, STALE), MonitorAvailability::Degraded);

        // 이후 구간에 새 미스가 없다 → 회복
        tick_us(&h, 2_000, 2_000, 10);
        assert_eq!(h.scan(NEAR, STALE), MonitorAvailability::Available);
        // 누적 횟수는 그대로 남는다 — 회복은 판정이지 기록 삭제가 아니다
        assert_eq!(h.group_stats(0).misses, 1);
    }

    #[test]
    fn miss_counted_while_another_group_is_stale_is_not_reported_twice() {
        let h = HealthRegistry::new(0, 2);
        // 그룹 1 은 시각 0 에 한 번 돌고 멈춘다
        h.record_group_tick(
            1,
            Instant::ZERO,
            Instant::ZERO,
            Instant::from_nanos(10),
            PERIOD,
        );
        tick_us(&h, 1_000, 1_995, 10); // 그룹 0 미스
        let late = Instant::from_nanos(200_000_000);
        assert_eq!(h.scan(late, STALE), MonitorAvailability::Unavailable);

        // 그룹 1 이 살아났고 그룹 0 에 새 미스는 없다 → 앞의 미스를 다시 세지 않는다
        let at = |ns: u64| Instant::from_nanos(ns);
        h.record_group_tick(1, at(200_000_000), at(200_000_000), at(200_000_010), PERIOD);
        h.record_group_tick(0, at(200_000_000), at(200_000_000), at(200_000_010), PERIOD);
        assert_eq!(h.scan(late, STALE), MonitorAvailability::Available);
    }

    #[test]
    fn stale_group_outranks_a_miss() {
        let h = HealthRegistry::new(0, 1);
        tick_us(&h, 1_000, 1_995, 10);
        let later = Instant::from_nanos(200_000_000);
        assert_eq!(h.scan(later, STALE), MonitorAvailability::Unavailable);
    }
}
