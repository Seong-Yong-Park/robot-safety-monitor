//! 리플레이 시험 — 적어 둔 입력을 가상 시각으로 재생하고, 출력 전체를 스냅샷으로 고정한다
//! (R-10, PRD D28).
//!
//! 스냅샷은 `tests/snapshots/`의 파일이다. 모듈·Arbiter·Supervisor 어디를 고쳐서든 출력이
//! 한 글자라도 달라지면 시험이 실패하고 차이를 보여 준다. 의도한 변화라면 스냅샷을 새로
//! 받고(`INSTA_UPDATE=always cargo test -p rsm-modules --test replay`), **diff 를 읽고 나서**
//! 커밋한다 — 스냅샷을 눈으로 확인하지 않고 갱신하면 이 시험은 아무것도 지키지 않는다.
//!
//! 레퍼런스 Detector 4종이 모두 여기서 재생된다: `threshold`·`min_distance`·
//! `comm_timeout`(시나리오 A), `panic_probe`(시나리오 B). 합성 Source(`ramp`·`constant`·
//! `stalling`)는 스스로 값을 만들어 리플레이 입력을 받지 않는다.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rsm_core::arbiter::MaxSeverity;
use rsm_core::config::PipelineConfig;
use rsm_core::registry::Registry;
use rsm_core::schedule;
use rsm_core::time::Duration;
use rsm_modules::sinks::JsonlSink;

/// 메모리에 모으는 출력.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

/// 설정을 `until_ms`까지 재생하고 JSONL 출력 전체를 돌려준다.
fn replay(cfg: PipelineConfig, until_ms: u64) -> String {
    let mut reg = Registry::new();
    rsm_modules::register_all(&mut reg);
    let built = cfg.build(&reg).expect("the pipeline must build");
    let buf = Buf::default();
    schedule::replay(
        built,
        Box::new(MaxSeverity::new()),
        Box::new(JsonlSink::to_writer(Box::new(buf.clone()))),
        Duration::from_millis(until_ms),
    );
    let bytes = buf.0.lock().unwrap().clone();
    String::from_utf8(bytes).expect("JSONL is UTF-8")
}

fn scenario_a() -> PipelineConfig {
    PipelineConfig::from_path(&examples().join("replay-a.yaml")).expect("replay-a.yaml parses")
}

/// 시나리오 A: 토크 초과 → 장애물 경고 → 치명 → 해소 → 바퀴 신호 끊김.
/// 입력은 `examples/replay-a.jsonl`, 설정은 `examples/replay-a.yaml`.
#[test]
fn scenario_a_threshold_min_distance_comm_timeout() {
    insta::assert_snapshot!("scenario_a", replay(scenario_a(), 1_500));
}

/// 시나리오 B: 같은 입력에 일부러 패닉하는 모듈을 더한다. 그 모듈만 빠지고(`Faulted`)
/// 나머지는 계속 판정하며, 감시 범위가 줄었다는 사실이 `Degraded`로 출력된다 (R-07).
#[test]
fn scenario_b_panic_is_isolated_and_reported() {
    let yaml = r#"
version: 1
groups:
  control: { period_hz: 100 }
modules:
  - name: recording
    type: replay
    group: control
    params: { path: replay-a.jsonl }
  - name: torque_limit
    type: threshold
    group: control
    params:
      signal: joint.torque
      limit: 100.0
      hazard: internal.joint.torque_limit
      severity: critical
      ttl_ms: 50
  - name: probe
    type: panic_probe
    group: control
    params: { after_ticks: 30 }       # 30 번째 tick = 290 ms 에 패닉
arbiter: { period_hz: 20 }
supervisor: { period_hz: 10, group_stale_after_ms: 500 }
sink: { format: jsonl, path: "-", on_change_only: true }
recommended_action:
  by_level: { advisory: none, warning: slow_down, critical: stop }
  on_unavailable: stop
"#;
    let mut cfg = PipelineConfig::from_str(yaml).unwrap();
    cfg.base_dir = Some(examples());
    insta::assert_snapshot!("scenario_b", replay(cfg, 1_000));
}

/// PRD §7: 같은 입력을 10 번 재생하면 출력이 같다.
#[test]
fn ten_replays_produce_identical_output() {
    let first = replay(scenario_a(), 1_500);
    assert!(!first.is_empty());
    for _ in 1..10 {
        assert_eq!(replay(scenario_a(), 1_500), first);
    }
}

/// 입력 파일을 못 찾으면 기동하지 않는다. 경로는 설정 파일 기준으로 풀린다.
#[test]
fn a_missing_input_file_is_a_config_error() {
    let mut cfg = scenario_a();
    cfg.base_dir = Some(PathBuf::from("/nonexistent"));
    let mut reg = Registry::new();
    rsm_modules::register_all(&mut reg);
    let err = cfg.build(&reg).unwrap_err().to_string();
    assert!(err.contains("cannot read replay file"), "{err}");
}
