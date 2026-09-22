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
