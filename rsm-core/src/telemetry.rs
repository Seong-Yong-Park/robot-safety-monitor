//! 계측 배출 — 건강 표의 통계를 [`metrics`] 파사드로 옮긴다 (TECH_STACK T7의 (c)).
//!
//! tick 경로는 [`HealthRegistry`]의 원자 슬롯에만 쓴다. 이 모듈이 **Supervisor 스레드**에서
//! scan 마다 그 값을 읽어 파사드로 넘긴다. 펌웨어로 치면 ISR은 카운터만 올리고, 배경
//! 루프가 그 값을 UART로 내보내는 구조다 — 내보내는 쪽이 느리거나 막혀도 tick 은 모른다.
//!
//! 파사드라 **어디로 갈지는 실행 파일이 정한다**(레코더 설치). 아무것도 설치하지 않으면
//! 핸들은 아무 일도 하지 않는다. v1의 레코더는 종료 시 요약 JSON(`rsm --metrics-json`),
//! Prometheus는 P1(R-14)이다.
//!
//! `metrics` 매크로는 이 파일 밖에서 쓰지 못한다(clippy `disallowed_macros`). 1 kHz 루프에
//! 매크로가 들어가면 레코더에 따라 해시·락·할당이 tick 경로에 생긴다.
//!
//! | 이름 | 종류 | 라벨 | 뜻 |
//! |---|---|---|---|
//! | `rsm_availability` | gauge | — | 0 `Available` · 1 `Degraded` · 2 `Unavailable` |
//! | `rsm_group_ticks_total` | counter | `group` | 실행한 tick 수 |
//! | `rsm_group_deadline_misses_total` | counter | `group` | 데드라인 미스 수 |
//! | `rsm_group_queue_overflows_total` | counter | `group` | 큐가 가득 차 버린 이벤트 수 |
//! | `rsm_group_late_p50_ns` · `_p99_ns` · `_p999_ns` · `_max_ns` | gauge | `group` | 시작 지연(지터) |
//! | `rsm_group_work_max_ns` | gauge | `group` | 가장 긴 작업 시간 |
//! | `rsm_module_faults_total` | counter | `module` | 패닉 횟수 |
//! | `rsm_module_health` | gauge | `module` | 0 `Ok` · 1 `Degraded` · 2 `Faulted` |

#![allow(
    clippy::disallowed_macros,
    reason = "this module is the only place allowed to touch the metrics facade (TECH_STACK T7)"
)]

use metrics::{Counter, Gauge, Unit, counter, describe_counter, describe_gauge, gauge};

use crate::event::{ModuleId, MonitorAvailability};
use crate::intern::Interner;
use crate::supervisor::HealthRegistry;
use crate::traits::Health;

struct GroupHandles {
    ticks: Counter,
    misses: Counter,
    overflows: Counter,
    late_p50: Gauge,
    late_p99: Gauge,
    late_p999: Gauge,
    late_max: Gauge,
    work_max: Gauge,
}

struct ModuleHandles {
    id: ModuleId,
    faults: Counter,
    health: Gauge,
}

/// 건강 표 → 파사드. 핸들은 만들 때 한 번 등록하고, [`publish`](Self::publish)는 값만 쓴다.
///
/// 만들 때 설치되어 있던 레코더에 묶인다. 레코더를 나중에 설치하면 이 값은 그쪽으로 가지 않는다.
pub struct MetricsExporter {
    availability: Gauge,
    groups: Vec<GroupHandles>,
    modules: Vec<ModuleHandles>,
}

impl std::fmt::Debug for MetricsExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsExporter")
            .field("groups", &self.groups.len())
            .field("modules", &self.modules.len())
            .finish_non_exhaustive()
    }
}

impl MetricsExporter {
    /// 그룹 이름(그룹 번호 순)과 모듈 이름표로 핸들을 등록한다.
    pub fn new(group_names: &[String], module_names: &Interner) -> Self {
        describe();
        let groups = group_names
            .iter()
            .map(|g| {
                let g = g.clone();
                GroupHandles {
                    ticks: counter!("rsm_group_ticks_total", "group" => g.clone()),
                    misses: counter!("rsm_group_deadline_misses_total", "group" => g.clone()),
                    overflows: counter!("rsm_group_queue_overflows_total", "group" => g.clone()),
                    late_p50: gauge!("rsm_group_late_p50_ns", "group" => g.clone()),
                    late_p99: gauge!("rsm_group_late_p99_ns", "group" => g.clone()),
                    late_p999: gauge!("rsm_group_late_p999_ns", "group" => g.clone()),
                    late_max: gauge!("rsm_group_late_max_ns", "group" => g.clone()),
                    work_max: gauge!("rsm_group_work_max_ns", "group" => g),
                }
            })
            .collect();
        let modules = (0..module_names.len())
            .filter_map(|i| u16::try_from(i).ok())
            .map(|id| {
                let name = module_names.name(id).unwrap_or("?").to_owned();
                ModuleHandles {
                    id: ModuleId(id),
                    faults: counter!("rsm_module_faults_total", "module" => name.clone()),
                    health: gauge!("rsm_module_health", "module" => name),
                }
            })
            .collect();
        Self {
            availability: gauge!("rsm_availability"),
            groups,
            modules,
        }
    }

    /// 지금 건강 표의 값을 내보낸다. Supervisor 가 scan 직후에 부른다.
    #[allow(
        clippy::cast_precision_loss,
        reason = "gauges are f64 by the facade's design; ns values stay far below 2^53"
    )]
    pub fn publish(&self, health: &HealthRegistry) {
        self.availability.set(match health.availability() {
            MonitorAvailability::Available => 0.0,
            MonitorAvailability::Degraded => 1.0,
            MonitorAvailability::Unavailable => 2.0,
        });
        for (i, g) in self.groups.iter().enumerate() {
            let s = health.group_stats(i);
            g.ticks.absolute(s.ticks);
            g.misses.absolute(s.misses);
            g.overflows.absolute(s.overflows);
            g.late_p50.set(s.p50_late_ns as f64);
            g.late_p99.set(s.p99_late_ns as f64);
            g.late_p999.set(s.p999_late_ns as f64);
            g.late_max.set(s.max_late_ns as f64);
            g.work_max.set(s.max_work_ns as f64);
        }
        for m in &self.modules {
            m.faults.absolute(health.module_faults(m.id));
            m.health.set(match health.module_health(m.id) {
                Health::Ok => 0.0,
                Health::Degraded => 1.0,
                Health::Faulted => 2.0,
            });
        }
    }
}

/// 이름마다 설명과 단위를 단다. 레코더가 원하면 보여 준다(Prometheus 의 `# HELP` 등).
fn describe() {
    describe_gauge!(
        "rsm_availability",
        "monitoring availability: 0 Available, 1 Degraded, 2 Unavailable"
    );
    describe_counter!("rsm_group_ticks_total", "ticks executed by the group");
    describe_counter!(
        "rsm_group_deadline_misses_total",
        "ticks that ended after release + period"
    );
    describe_counter!(
        "rsm_group_queue_overflows_total",
        "events dropped because the group's queue was full"
    );
    for (name, what) in [
        ("rsm_group_late_p50_ns", "P50"),
        ("rsm_group_late_p99_ns", "P99"),
        ("rsm_group_late_p999_ns", "P99.9"),
        ("rsm_group_late_max_ns", "maximum"),
    ] {
        describe_gauge!(
            name,
            Unit::Nanoseconds,
            format!("start latency (jitter) {what}; a histogram bucket's upper bound")
        );
    }
    describe_gauge!(
        "rsm_group_work_max_ns",
        Unit::Nanoseconds,
        "longest tick work time"
    );
    describe_counter!("rsm_module_faults_total", "module panics caught");
    describe_gauge!(
        "rsm_module_health",
        "module health: 0 Ok, 1 Degraded, 2 Faulted"
    );
}
