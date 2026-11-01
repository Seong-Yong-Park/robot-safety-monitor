//! R-08 검증 — `fast` 그룹 tick 경로는 힙을 한 번도 건드리지 않는다.
//!
//! 실제 `group_loop`는 `sleep`이 있는 무한 루프라 그대로 부를 수 없다. 그 안에서
//! 수면만 뺀 나머지를 그대로 밟는다.
//!
//! ```text
//!   GroupWorker::tick   — Source 전부 poll → Detector 전부 tick → QueueSink 로 push
//!   record_group_tick   — 주기 통계 원자 변수
//! ```
//!
//! 레퍼런스 모듈을 전부 한 그룹에 넣는다. 지금 demo 의 fast 구성만이 아니라 어떤
//! 모듈이든 fast 그룹에 들어갈 수 있기 때문이다. 할당은 보통 처음 가는 분기에
//! 숨어 있으므로, 측정 구간이 아래 상태 전이를 **모두** 지나는지도 함께 확인한다.
//!
//! - 이벤트를 안 내는 tick / 내는 tick (`threshold`가 한계를 넘었다 내려왔다)
//! - 경고와 치명 두 단계 (`min_distance`)
//! - 큐가 가득 차 이벤트를 버림 (오버플로)
//! - 입력이 멈춤 → 끊김 감지 (`stalling` → `comm_timeout`)
//!
//! **범위 밖**: 패닉 경로. `catch_unwind`는 패닉 내용을 `Box`에 담으므로 반드시
//! 할당한다. 패닉은 정상 경로가 아니라 고장 경로이고 그 모듈은 `Faulted`로 빠진다.
//! Arbiter·Sink 경로(JSON 직렬화)도 R-08 대상이 아니다.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]

use std::sync::Arc;

use rsm_alloc_check::{Counts, count};
use rsm_core::config::PipelineConfig;
use rsm_core::event::{HazardEvent, HazardTypeId, Severity};
use rsm_core::registry::Registry;
use rsm_core::schedule::{GroupWorker, QueueSink};
use rsm_core::supervisor::HealthRegistry;
use rsm_core::time::{Duration, Instant};
use rtrb::RingBuffer;

const YAML: &str = r#"
version: 1
groups:
  fast:
    period_hz: 1000
    queue_capacity: 64
modules:
  # ── Source 전부 ──
  - name: torque
    type: ramp
    group: fast
    params: { signal: joint.torque, min: 0.0, max: 120.0, step: 1.0 }
  - name: lidar
    type: ramp
    group: fast
    params: { signal: obstacle.distance, min: 0.2, max: 3.0, step: 0.05 }
  - name: bias
    type: constant
    group: fast
    params: { signal: imu.bias, value: 0.5 }
  - name: encoder
    type: stalling
    group: fast
    params: { signal: wheel.speed, value: 0.8, stall_after: 150 }
  # ── Detector 전부 (panic_probe 제외) ──
  - name: torque_limit
    type: threshold
    group: fast
    params:
      signal: joint.torque
      limit: 100.0
      hazard: internal.joint.torque_limit
      severity: critical
      ttl_ms: 50
  - name: bias_floor
    type: threshold
    group: fast
    params:
      signal: imu.bias
      limit: 1.0
      direction: below
      hazard: internal.imu.bias_low
      severity: warning
      ttl_ms: 50
  - name: proximity
    type: min_distance
    group: fast
    params:
      signal: obstacle.distance
      warn_m: 1.0
      critical_m: 0.4
      hazard: external.min_distance
      ttl_ms: 50
  - name: encoder_link
    type: comm_timeout
    group: fast
    params:
      signal: wheel.speed
      timeout_ms: 20
      hazard: internal.comm_timeout
      ttl_ms: 50
arbiter: { period_hz: 20 }
supervisor: { period_hz: 10, group_stale_after_ms: 500 }
sink: { format: jsonl, path: "-", on_change_only: true }
"#;

/// 1 kHz 로 1.2 초. 톱니파가 여러 번 돌고, 입력이 멈춘 뒤 끊김 감지까지 지난다.
///
/// 워밍업 없이 **첫 tick부터** 잰다. R-08은 "초기화 이후"만 요구하지만, 첫 이벤트나
/// 첫 오버플로 때만 할당하는 지연 초기화가 있다면 그것도 운영 중 할당이므로 잡는다.
const MEASURED: u64 = 1_200;
/// Arbiter 가 큐를 비우는 간격(tick). 그 사이에 큐가 차서 오버플로가 난다.
const DRAIN_EVERY: u64 = 100;

#[test]
fn fast_tick_path_never_touches_the_heap() {
    let mut reg = Registry::new();
    rsm_modules::register_all(&mut reg);
    let mut built = PipelineConfig::from_str(YAML)
        .expect("the config must parse")
        .build(&reg)
        .expect("the pipeline must build");

    let hazard =
        |name: &str| HazardTypeId(built.names.hazards.get(name).expect("hazard is interned"));
    let torque_kind = hazard("internal.joint.torque_limit");
    let distance_kind = hazard("external.min_distance");
    let comm_kind = hazard("internal.comm_timeout");

    // ── 초기화: 여기까지는 할당해도 된다 ─────────────────────────────────────
    let plan = built.groups.remove(0);
    let (tx, mut rx) = RingBuffer::<HazardEvent>::new(plan.queue_capacity);
    let health = Arc::new(HealthRegistry::new(built.module_count, 1));
    let mut out = QueueSink::new(tx, 0, Arc::clone(&health));
    let mut worker = GroupWorker::new(plan, built.signal_count, 0);
    let period = worker.period();

    let mut offenders: Vec<(u64, Counts)> = Vec::new();
    let mut saw_torque = false;
    let mut saw_warning = false;
    let mut saw_critical_distance = false;
    let mut saw_comm = false;

    // ── 운영: tick 마다 구간을 열고 잰다 ─────────────────────────────────────
    for k in 0..MEASURED {
        let release = Instant::from_nanos(k * period.as_nanos());
        let ended = release + Duration::from_micros(5);

        let ((), n) = count(|| {
            worker.tick(release, &health, &mut out);
            health.record_group_tick(0, release, release, ended, period);
        });
        if !n.is_zero() {
            offenders.push((k, n));
        }

        // Arbiter 쪽 일이다. 측정 구간 밖에서 한다.
        if k % DRAIN_EVERY == DRAIN_EVERY - 1 {
            while let Ok(ev) = rx.pop() {
                saw_torque |= ev.kind == torque_kind;
                saw_comm |= ev.kind == comm_kind;
                if ev.kind == distance_kind {
                    saw_warning |= ev.severity == Severity::Warning;
                    saw_critical_distance |= ev.severity == Severity::Critical;
                }
            }
        }
    }

    // 측정 구간이 정말 모든 분기를 지났는가 — 아니면 0 은 아무 의미가 없다
    assert!(saw_torque, "threshold never fired");
    assert!(
        saw_warning && saw_critical_distance,
        "min_distance missed a level"
    );
    assert!(saw_comm, "comm_timeout never fired");
    assert!(
        health.group_stats(0).overflows > 0,
        "the queue never overflowed"
    );

    assert!(
        offenders.is_empty(),
        "{} of {MEASURED} ticks touched the heap; first few (tick, counts): {:?}",
        offenders.len(),
        &offenders[..offenders.len().min(5)]
    );
}
