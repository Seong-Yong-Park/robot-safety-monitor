//! 통합 시험 — YAML 한 장이 파이프라인이 되는지 끝에서 끝까지 확인한다.
//!
//! 스레드를 띄우지 않고 [`VirtualClock`]으로 tick을 손으로 밟는다. 실제 시간에
//! 기대지 않으므로 CI에서 결과가 흔들리지 않는다 (R-17 리플레이 결정성과 같은 원리).

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]

use rsm_core::arbiter::MaxSeverity;
use rsm_core::config::PipelineConfig;
use rsm_core::event::{MonitorAvailability, Severity};
use rsm_core::registry::Registry;
use rsm_core::schedule::{GroupWorker, VecSink};
use rsm_core::supervisor::HealthRegistry;
use rsm_core::time::{Clock, Duration, VirtualClock};
use rsm_core::traits::{ArbiterPolicy, Health};

use std::sync::Arc;

use rsm_core::error::ConfigError;
use rsm_core::event::SafetyState;
use rsm_core::intern::Names;
use rsm_core::schedule;
use rsm_core::time::MonotonicClock;
use rsm_core::traits::Sink;

const YAML: &str = r#"
version: 1
groups:
  fast:
    period_hz: 1000
    queue_capacity: 64
modules:
  - name: joint_torque
    type: ramp
    group: fast
    params: { signal: joint.torque, min: 0.0, max: 120.0, step: 1.0 }
  - name: torque_limit
    type: threshold
    group: fast
    params:
      signal: joint.torque
      limit: 100.0
      hazard: internal.joint.torque_limit
      severity: critical
      ttl_ms: 50
arbiter: { period_hz: 20 }
supervisor: { period_hz: 10, group_stale_after_ms: 500 }
sink: { format: jsonl, path: "-", on_change_only: true }
"#;

fn registry() -> Registry {
    let mut reg = Registry::new();
    rsm_modules::register_all(&mut reg);
    reg
}

#[test]
fn yaml_becomes_a_running_pipeline() {
    let built = PipelineConfig::from_str(YAML)
        .expect("the config must parse")
        .build(&registry())
        .expect("the pipeline must build");

    assert_eq!(built.groups.len(), 1);
    assert_eq!(built.module_count, 2);
    assert_eq!(built.signal_count, 1);

    let signal_count = built.signal_count;
    let mut groups = built.groups;
    let plan = groups.remove(0);
    let period = plan.period;
    let mut worker = GroupWorker::new(plan, signal_count, 0);
    let health = HealthRegistry::new(built.module_count, 1);
    let clock = VirtualClock::new();
    let mut policy = MaxSeverity::new();
    let mut out = VecSink::new();

    // 101 tick: ramp 가 0 → 100 까지 오른다. 101번째에 101.0 이 되어 한계를 넘는다.
    for _ in 0..102 {
        worker.tick(clock.now(), &health, &mut out);
        clock.advance(period);
    }

    assert!(
        !out.events.is_empty(),
        "crossing the limit must produce an event"
    );
    let first = out.events[0];
    assert_eq!(first.severity, Severity::Critical);

    // 이름표를 펼치면 설정에 쓴 문자열이 그대로 돌아온다 — 인터닝의 역방향.
    let kind = built.names.hazards.name(first.kind.0).unwrap();
    assert_eq!(kind, "internal.joint.torque_limit");
    let src = built.names.modules.name(first.source.0).unwrap();
    assert_eq!(src, "torque_limit");

    for e in &out.events {
        policy.ingest(*e);
    }
    let state = policy.evaluate(clock.now(), MonitorAvailability::Available);
    assert_eq!(state.level, Severity::Critical);

    // TTL 이 지나면 아무도 갱신하지 않아도 저절로 사라진다. 해제 메시지는 없다.
    clock.advance(Duration::from_millis(500));
    let later = policy.evaluate(clock.now(), MonitorAvailability::Available);
    assert_eq!(later.level, Severity::None);
    assert!(later.active.is_empty());

    assert_eq!(
        health.module_health(rsm_core::event::ModuleId(0)),
        Health::Ok
    );
}

#[test]
fn unconnected_input_is_rejected_at_load_time() {
    let yaml = r#"
version: 1
groups:
  fast: { period_hz: 100 }
modules:
  - name: torque_limit
    type: threshold
    group: fast
    params: { signal: joint.torque, limit: 100.0 }
arbiter: { period_hz: 20 }
supervisor: { period_hz: 10, group_stale_after_ms: 500 }
sink: { format: jsonl, path: "-" }
"#;
    // 아무도 joint.torque 를 내지 않는다. 기동 자체가 실패해야 한다 —
    // 돌다가 조용히 아무 판정도 못 하는 것보다 낫다.
    let err = PipelineConfig::from_str(yaml)
        .expect("the YAML itself is well formed")
        .build(&registry())
        .expect_err("an unconnected input must be rejected");
    assert!(err.to_string().contains("joint.torque"), "{err}");
}

#[test]
fn unknown_module_type_is_rejected() {
    let yaml = r#"
version: 1
groups:
  fast: { period_hz: 100 }
modules:
  - name: x
    type: no_such_module
    group: fast
    params: {}
arbiter: { period_hz: 20 }
supervisor: { period_hz: 10, group_stale_after_ms: 500 }
sink: { format: jsonl, path: "-" }
"#;
    let err = PipelineConfig::from_str(yaml)
        .expect("the YAML itself is well formed")
        .build(&registry())
        .expect_err("an unknown module type must be rejected");
    assert!(err.to_string().contains("no_such_module"), "{err}");
}

// ── 설정 검증: 주기·우선순위 (TECH_STACK T5) ─────────────────────────────────

/// `YAML`의 fast 그룹 줄을 바꿔 끼운 설정.
fn with_group(group_line: &str) -> String {
    YAML.replace(
        "  fast:\n    period_hz: 1000\n    queue_capacity: 64\n",
        group_line,
    )
}

fn build_err(yaml: &str) -> String {
    PipelineConfig::from_str(yaml)
        .expect("the YAML itself is well formed")
        .build(&registry())
        .expect_err("the config must be rejected")
        .to_string()
}

#[test]
fn rt_priority_outside_1_to_99_is_rejected() {
    for p in [0, 100] {
        let yaml = with_group(&format!(
            "  fast:\n    period_hz: 1000\n    rt_priority: {p}\n"
        ));
        let err = build_err(&yaml);
        assert!(err.contains("rt_priority"), "{err}");
    }
}

#[test]
fn rt_priority_is_carried_into_the_plan() {
    let yaml = with_group("  fast:\n    period_hz: 1000\n    rt_priority: 80\n");
    let built = PipelineConfig::from_str(&yaml)
        .unwrap()
        .build(&registry())
        .unwrap();
    assert_eq!(built.groups[0].rt_priority, Some(80));

    let built = PipelineConfig::from_str(YAML)
        .unwrap()
        .build(&registry())
        .unwrap();
    assert_eq!(built.groups[0].rt_priority, None, "absent means OS default");
}

#[test]
fn zero_hz_is_rejected_instead_of_silently_becoming_10_hz() {
    let err = build_err(&with_group("  fast:\n    period_hz: 0\n"));
    assert!(err.contains("period_hz"), "{err}");

    let err = build_err(&YAML.replace("arbiter: { period_hz: 20 }", "arbiter: { period_hz: 0 }"));
    assert!(err.contains("period_hz"), "{err}");
}

#[test]
fn arbiter_slower_than_supervisor_is_rejected() {
    // Supervisor 10 Hz 가 100 ms 동안 낮춘 Degraded 를 5 Hz Arbiter 는 못 보고 지나칠 수 있다
    let err = build_err(&YAML.replace("arbiter: { period_hz: 20 }", "arbiter: { period_hz: 5 }"));
    assert!(err.contains("supervisor"), "{err}");

    // 같으면 된다
    let yaml = YAML.replace("arbiter: { period_hz: 20 }", "arbiter: { period_hz: 10 }");
    assert!(
        PipelineConfig::from_str(&yaml)
            .unwrap()
            .build(&registry())
            .is_ok()
    );
}

// ── 실제 스레드로 기동·정지 ───────────────────────────────────────────────────

struct NullSink;
impl Sink for NullSink {
    fn publish(&mut self, _state: &SafetyState, _names: &Names) {}
}

fn start(yaml: &str) -> Result<schedule::Runtime, ConfigError> {
    let built = PipelineConfig::from_str(yaml)
        .unwrap()
        .build(&registry())
        .unwrap();
    let clock: Arc<dyn Clock> = Arc::new(MonotonicClock::new());
    schedule::run(
        built,
        &clock,
        Box::new(MaxSeverity::new()),
        Box::new(NullSink),
    )
}

#[test]
fn run_starts_ticks_and_stops() {
    let rt = start(YAML).expect("no rt_priority — nothing can fail here");
    std::thread::sleep(std::time::Duration::from_millis(30));
    rt.stop();
    let ticks = rt.health().group_stats(0).ticks;
    rt.join();
    assert!(ticks > 0, "the group thread never ticked");
}

/// 권한에 따라 결과가 갈린다. root 면 적용되고, 아니면 기동이 **분명하게** 실패해야
/// 한다. 어느 쪽이든 "우선순위 없이 조용히 도는" 경우는 없다.
#[test]
fn rt_priority_either_applies_or_fails_loudly() {
    let yaml = with_group("  fast:\n    period_hz: 1000\n    rt_priority: 10\n");
    match start(&yaml) {
        Ok(rt) => {
            rt.stop();
            rt.join();
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(msg.contains("rsm-group-fast"), "{msg}");
            assert!(
                msg.contains("permission denied") || msg.contains("only on Linux"),
                "{msg}"
            );
        }
    }
}
