//! 계측 파사드 시험 — 재생한 파이프라인의 통계가 레코더까지 그대로 가는지 (TECH_STACK T7).
//!
//! 가상 시각 재생이라 tick 수까지 정확히 맞출 수 있다. 레코더는 시험 스레드에만 설치한다
//! (`with_local_recorder`) — 재생은 스레드를 띄우지 않으므로 이것으로 충분하고, 다른
//! 시험과 섞이지 않는다.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "in tests, a failed assertion is the diagnostic; compared values are small integers"
)]

use std::path::PathBuf;

use rsm_core::arbiter::MaxSeverity;
use rsm_core::config::PipelineConfig;
use rsm_core::event::SafetyState;
use rsm_core::intern::Names;
use rsm_core::registry::Registry;
use rsm_core::schedule;
use rsm_core::time::Duration;
use rsm_core::traits::Sink;
use rsm_modules::metrics_summary::SummaryRecorder;

struct NullSink;
impl Sink for NullSink {
    fn publish(&mut self, _: &SafetyState, _: &Names) {}
}

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

/// 레코더를 걸고 재생한 뒤 레코더를 돌려준다.
fn replay_with_recorder(cfg: PipelineConfig, until_ms: u64) -> SummaryRecorder {
    let mut reg = Registry::new();
    rsm_modules::register_all(&mut reg);
    let built = cfg.build(&reg).unwrap();
    let rec = SummaryRecorder::new();
    metrics::with_local_recorder(&rec, || {
        schedule::replay(
            built,
            Box::new(MaxSeverity::new()),
            Box::new(NullSink),
            Duration::from_millis(until_ms),
        );
    });
    rec
}

/// `name{label=value}` 의 값.
fn value(rec: &SummaryRecorder, name: &str, label: Option<(&str, &str)>) -> f64 {
    rec.snapshot()
        .into_iter()
        .find(|(n, labels, _)| {
            n == name && label.is_none_or(|(k, v)| labels.get(k).map(String::as_str) == Some(v))
        })
        .map_or_else(
            || panic!("metric {name} {label:?} was not exported"),
            |(_, _, v)| v,
        )
}

#[test]
fn replay_statistics_reach_the_recorder() {
    let cfg = PipelineConfig::from_path(&examples().join("replay-a.yaml")).unwrap();
    let rec = replay_with_recorder(cfg, 1_500);

    // 100 Hz 로 0 ms ~ 1500 ms → 151 tick
    assert_eq!(
        value(&rec, "rsm_group_ticks_total", Some(("group", "control"))),
        151.0
    );
    assert_eq!(
        value(
            &rec,
            "rsm_group_queue_overflows_total",
            Some(("group", "control"))
        ),
        0.0
    );
    assert_eq!(
        value(
            &rec,
            "rsm_group_deadline_misses_total",
            Some(("group", "control"))
        ),
        0.0
    );
    // 가상 시각이라 지연은 0
    assert_eq!(
        value(&rec, "rsm_group_late_p99_ns", Some(("group", "control"))),
        0.0
    );
    assert_eq!(value(&rec, "rsm_availability", None), 0.0);
    for m in ["recording", "torque_limit", "proximity", "wheel_link"] {
        assert_eq!(
            value(&rec, "rsm_module_faults_total", Some(("module", m))),
            0.0,
            "{m}"
        );
        assert_eq!(
            value(&rec, "rsm_module_health", Some(("module", m))),
            0.0,
            "{m}"
        );
    }
}

#[test]
fn a_panicking_module_shows_up_as_a_fault_and_degraded() {
    let yaml = r"
version: 1
groups:
  control: { period_hz: 100 }
modules:
  - name: recording
    type: replay
    group: control
    params: { path: replay-a.jsonl }
  - name: probe
    type: panic_probe
    group: control
    params: { after_ticks: 30 }
arbiter: { period_hz: 20 }
supervisor: { period_hz: 10, group_stale_after_ms: 500 }
";
    let mut cfg = PipelineConfig::from_str(yaml).unwrap();
    cfg.base_dir = Some(examples());
    let rec = replay_with_recorder(cfg, 500);

    assert_eq!(
        value(&rec, "rsm_module_faults_total", Some(("module", "probe"))),
        1.0
    );
    assert_eq!(
        value(&rec, "rsm_module_health", Some(("module", "probe"))),
        2.0
    ); // Faulted
    assert_eq!(
        value(&rec, "rsm_module_health", Some(("module", "recording"))),
        0.0
    );
    assert_eq!(value(&rec, "rsm_availability", None), 1.0); // Degraded
}

#[test]
fn the_summary_is_a_sorted_json_array() {
    let cfg = PipelineConfig::from_path(&examples().join("replay-a.yaml")).unwrap();
    let rec = replay_with_recorder(cfg, 100);
    let v: serde_json::Value = serde_json::from_str(&rec.to_json()).unwrap();
    let items = v.as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    // 카운터는 정수로 나간다(소수점 없이)
    let ticks = items
        .iter()
        .find(|i| i["name"] == "rsm_group_ticks_total")
        .unwrap();
    assert_eq!(ticks["kind"], "counter");
    assert_eq!(ticks["value"], serde_json::json!(11));
}
