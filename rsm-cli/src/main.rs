//! # rsm — Robot Safety Monitor 실행기
//!
//! 설정 파일 하나가 파이프라인 전부를 정한다. 이 바이너리는 **다시 컴파일되지
//! 않는다** — 조합이 달라지면 YAML만 바뀐다 (PRD G2).
//!
//! ```text
//! rsm check --config examples/demo-a.yaml    # 설정만 검증하고 끝
//! rsm run   --config examples/demo-a.yaml    # Ctrl-C 로 멈출 때까지
//! rsm run   --config examples/demo-a.yaml --duration-ms 2000
//! rsm replay --config examples/replay-a.yaml --duration-ms 1500   # 가상 시각, 즉시 끝남
//! rsm list                                   # 등록된 모듈 이름
//! ```
//!
//! Ctrl-C 는 `ctrlc` 크레이트가 받는다. 시그널 핸들러에는 `unsafe`가 필요한데, 그 일을
//! 이 크레이트에 맡기고 코어는 [`Runtime::stop`](rsm_core::schedule::Runtime::stop)만
//! 부른다. 의존성은 실행 파일에만 있다 (TECH_STACK T10).
//!
//! `replay`는 같은 설정을 벽시계 대신 가상 시각으로 돌린다
//! ([`schedule::replay`]). 입력은 설정 안의 `type: replay`
//! Source가 읽는 파일이다. 같은 입력이면 출력이 바이트 단위로 같다 (R-10).

use std::fmt::Write as _;
use std::io::{ErrorKind, Write};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rsm_core::arbiter::MaxSeverity;
use rsm_core::config::PipelineConfig;
use rsm_core::registry::Registry;
use rsm_core::schedule;
use rsm_core::time::MonotonicClock;
use rsm_modules::metrics_summary::SummaryRecorder;
use rsm_modules::sinks::JsonlSink;

const USAGE: &str = "\
rsm — Robot Safety Monitor

USAGE:
  rsm run    --config <FILE> [--duration-ms <MILLIS>]   (runs until Ctrl-C if no duration)
  rsm replay --config <FILE> --duration-ms <MILLIS>     (virtual time, as fast as possible)
  rsm check  --config <FILE>
  rsm list

OPTIONS (run, replay):
  --metrics-json <PATH>   write a metrics summary (JSON array) to PATH on exit
";

/// 아주 작은 인자 파서. 의존성을 하나 더 들이지 않으려고 손으로 썼다.
struct Args {
    config: Option<String>,
    /// 없으면 Ctrl-C 까지 돈다.
    duration_ms: Option<u64>,
    /// 종료 시 계측 요약을 쓸 파일 (TECH_STACK T7).
    metrics_json: Option<String>,
}

fn parse_args(rest: &[String]) -> Result<Args, String> {
    let mut args = Args {
        config: None,
        duration_ms: None,
        metrics_json: None,
    };
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--config" | "-c" => {
                let v = rest.get(i + 1).ok_or("--config requires a value")?;
                args.config = Some(v.clone());
                i += 2;
            }
            "--duration-ms" => {
                let v = rest.get(i + 1).ok_or("--duration-ms requires a value")?;
                args.duration_ms = Some(
                    v.parse()
                        .map_err(|_| format!("--duration-ms is not a number: {v}"))?,
                );
                i += 2;
            }
            "--metrics-json" => {
                let v = rest.get(i + 1).ok_or("--metrics-json requires a path")?;
                args.metrics_json = Some(v.clone());
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn registry() -> Registry {
    let mut reg = Registry::new();
    rsm_modules::register_all(&mut reg);
    reg
}

/// 표준출력에 쓴다. `rsm list | head -1`처럼 읽는 쪽이 먼저 닫으면(broken pipe)
/// 오류가 아니라 정상 종료로 본다. `println!`은 이 경우 패닉한다.
fn print_out(text: &str) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(e) if e.kind() != ErrorKind::BrokenPipe => Err(format!("cannot write to stdout: {e}")),
        _ => Ok(()),
    }
}

fn cmd_check(path: &str) -> Result<(), String> {
    let cfg = PipelineConfig::from_path(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let built = cfg.build(&registry()).map_err(|e| e.to_string())?;
    let mut text = format!(
        "OK  groups {} · modules {} · signals {}\n",
        built.groups.len(),
        built.module_count,
        built.signal_count
    );
    for g in &built.groups {
        let sched = g
            .rt_priority
            .map_or_else(|| "default".to_owned(), |p| format!("SCHED_FIFO {p}"));
        // String 에 쓰는 일은 실패하지 않는다
        let _ = writeln!(
            text,
            "    [{}] {:.1} Hz  source {} · detector {} · sched {sched}",
            g.name,
            1e9_f64 / f64::from(u32::try_from(g.period.as_nanos()).unwrap_or(u32::MAX)),
            g.sources.len(),
            g.detectors.len()
        );
    }
    print_out(&text)
}

fn cmd_run(path: &str, duration_ms: Option<u64>) -> Result<(), String> {
    let cfg = PipelineConfig::from_path(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let built = cfg.build(&registry()).map_err(|e| e.to_string())?;

    let sink = JsonlSink::open(&built.sink.path).map_err(|e| e.to_string())?;
    let clock: Arc<dyn rsm_core::time::Clock> = Arc::new(MonotonicClock::new());
    let group_names: Vec<String> = built.groups.iter().map(|g| g.name.clone()).collect();

    // 핸들러는 신호를 알리기만 한다. 멈추는 일은 이 스레드가 한다.
    let (interrupted_tx, interrupted) = mpsc::channel::<()>();
    ctrlc::set_handler(move || {
        let _ = interrupted_tx.send(());
    })
    .map_err(|e| format!("cannot install the Ctrl-C handler: {e}"))?;

    let rt = schedule::run(built, &clock, Box::new(MaxSeverity::new()), Box::new(sink))
        .map_err(|e| e.to_string())?;
    let by_ctrl_c = match duration_ms {
        Some(ms) => interrupted.recv_timeout(Duration::from_millis(ms)).is_ok(),
        None => interrupted.recv().is_ok(),
    };
    if by_ctrl_c {
        eprintln!("[rsm] Ctrl-C — stopping");
    }
    rt.stop();

    let stats: Vec<_> = (0..group_names.len())
        .map(|i| rt.health().group_stats(i))
        .collect();
    rt.join();
    // 한 그룹당 한 줄. tools/jitter.sh 가 이 형식을 파싱한다.
    for (name, s) in group_names.iter().zip(&stats) {
        eprintln!(
            "[rsm] {name} tick {} · {} Hz · miss {} · late p50 {} p99 {} p99.9 {} max {} mean {} µs · work max {} mean {} µs · overflow {}",
            s.ticks,
            one_decimal(s.effective_millihertz()),
            s.misses,
            one_decimal(s.p50_late_ns),
            one_decimal(s.p99_late_ns),
            one_decimal(s.p999_late_ns),
            one_decimal(s.max_late_ns),
            one_decimal(s.mean_late_ns),
            one_decimal(s.max_work_ns),
            one_decimal(s.mean_work_ns),
            s.overflows,
        );
    }
    Ok(())
}

/// 가상 시각으로 `duration_ms`까지 돌린다. 벽시계를 보지 않으므로 기다리지 않는다.
fn cmd_replay(path: &str, duration_ms: u64) -> Result<(), String> {
    let cfg = PipelineConfig::from_path(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let built = cfg.build(&registry()).map_err(|e| e.to_string())?;
    let sink = JsonlSink::open(&built.sink.path).map_err(|e| e.to_string())?;
    let group_names: Vec<String> = built.groups.iter().map(|g| g.name.clone()).collect();

    let health = schedule::replay(
        built,
        Box::new(MaxSeverity::new()),
        Box::new(sink),
        rsm_core::time::Duration::from_millis(duration_ms),
    );
    // 가상 시각이라 지연·작업 시간은 늘 0 이다. tick 수와 유실만 보인다.
    for (i, name) in group_names.iter().enumerate() {
        let s = health.group_stats(i);
        eprintln!(
            "[rsm] replay {name} tick {} · overflow {} ({duration_ms} ms virtual)",
            s.ticks, s.overflows
        );
    }
    Ok(())
}

/// 1/1000 단위 정수를 소수 한 자리로 찍는다 (ns → µs, mHz → Hz).
fn one_decimal(milli: u64) -> String {
    format!("{}.{}", milli / 1_000, milli % 1_000 / 100)
}

fn cmd_list() -> Result<(), String> {
    let reg = registry();
    let mut text = String::from("source:\n");
    for n in reg.source_names() {
        let _ = writeln!(text, "  {n}");
    }
    text += "detector:\n";
    for n in reg.detector_names() {
        let _ = writeln!(text, "  {n}");
    }
    print_out(&text)
}

fn dispatch(argv: &[String]) -> Result<(), String> {
    let Some(sub) = argv.first() else {
        return Err(USAGE.to_owned());
    };
    match sub.as_str() {
        "list" => cmd_list(),
        "check" | "run" | "replay" => {
            let opts = parse_args(&argv[1..])?;
            let path = opts.config.ok_or("--config is required")?;
            if sub == "check" && opts.metrics_json.is_some() {
                return Err("--metrics-json applies to run and replay only".to_owned());
            }
            // 레코더는 파이프라인이 계측 핸들을 등록하기 **전에** 설치해야 한다.
            let recorder = opts.metrics_json.as_ref().map(|_| SummaryRecorder::new());
            if let Some(r) = &recorder {
                r.install_global()?;
            }
            match sub.as_str() {
                "check" => cmd_check(&path)?,
                "run" => cmd_run(&path, opts.duration_ms)?,
                // 재생은 끝을 정해야 한다. 파일이 끝난 뒤의 낡음·끊김도 결과의 일부다.
                _ => cmd_replay(
                    &path,
                    opts.duration_ms
                        .ok_or("replay requires --duration-ms (virtual time)")?,
                )?,
            }
            if let (Some(r), Some(out)) = (&recorder, &opts.metrics_json) {
                std::fs::write(out, r.to_json() + "\n")
                    .map_err(|e| format!("cannot write metrics summary to `{out}`: {e}"))?;
            }
            Ok(())
        }
        "--help" | "-h" | "help" => print_out(USAGE),
        other => Err(format!("unknown command: {other}\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
