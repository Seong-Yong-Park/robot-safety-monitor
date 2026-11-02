//! # rsm — Robot Safety Monitor 실행기
//!
//! 설정 파일 하나가 파이프라인 전부를 정한다. 이 바이너리는 **다시 컴파일되지
//! 않는다** — 조합이 달라지면 YAML만 바뀐다 (PRD G2).
//!
//! ```text
//! rsm check --config examples/demo-a.yaml    # 설정만 검증하고 끝
//! rsm run   --config examples/demo-a.yaml --duration-ms 2000
//! rsm list                                   # 등록된 모듈 이름
//! ```
//!
//! Phase 0에서는 Ctrl-C 처리를 넣지 않았다. 시그널 핸들러는 외부 크레이트나
//! `unsafe`가 필요해 [T10 규칙](../../docs/TECH_STACK.md)과 충돌하기 때문이다.
//! 대신 `--duration-ms` 로 실행 시간을 정한다. Phase 1에서 다시 본다.

mod jsonl;

use std::process::ExitCode;
use std::sync::Arc;

use rsm_core::arbiter::MaxSeverity;
use rsm_core::config::PipelineConfig;
use rsm_core::registry::Registry;
use rsm_core::schedule;
use rsm_core::time::MonotonicClock;

use crate::jsonl::JsonlSink;

const USAGE: &str = "\
rsm — Robot Safety Monitor

USAGE:
  rsm run   --config <FILE> [--duration-ms <MILLIS>]
  rsm check --config <FILE>
  rsm list
";

/// 아주 작은 인자 파서. 의존성을 하나 더 들이지 않으려고 손으로 썼다.
struct Args {
    config: Option<String>,
    duration_ms: u64,
}

fn parse_args(rest: &[String]) -> Result<Args, String> {
    let mut args = Args {
        config: None,
        duration_ms: 3_000,
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
                args.duration_ms = v
                    .parse()
                    .map_err(|_| format!("--duration-ms is not a number: {v}"))?;
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

fn cmd_check(path: &str) -> Result<(), String> {
    let cfg = PipelineConfig::from_path(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let built = cfg.build(&registry()).map_err(|e| e.to_string())?;
    println!(
        "OK  groups {} · modules {} · signals {}",
        built.groups.len(),
        built.module_count,
        built.signal_count
    );
    for g in &built.groups {
        let sched = g
            .rt_priority
            .map_or_else(|| "default".to_owned(), |p| format!("SCHED_FIFO {p}"));
        println!(
            "    [{}] {:.1} Hz  source {} · detector {} · sched {sched}",
            g.name,
            1e9_f64 / f64::from(u32::try_from(g.period.as_nanos()).unwrap_or(u32::MAX)),
            g.sources.len(),
            g.detectors.len()
        );
    }
    Ok(())
}

fn cmd_run(path: &str, duration_ms: u64) -> Result<(), String> {
    let cfg = PipelineConfig::from_path(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let built = cfg.build(&registry()).map_err(|e| e.to_string())?;

    let sink = JsonlSink::open(&built.sink.path).map_err(|e| e.to_string())?;
    let clock: Arc<dyn rsm_core::time::Clock> = Arc::new(MonotonicClock::new());
    let group_names: Vec<String> = built.groups.iter().map(|g| g.name.clone()).collect();

    let rt = schedule::run(built, &clock, Box::new(MaxSeverity::new()), Box::new(sink))
        .map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
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

/// 1/1000 단위 정수를 소수 한 자리로 찍는다 (ns → µs, mHz → Hz).
fn one_decimal(milli: u64) -> String {
    format!("{}.{}", milli / 1_000, milli % 1_000 / 100)
}

fn cmd_list() {
    let reg = registry();
    println!("source:");
    for n in reg.source_names() {
        println!("  {n}");
    }
    println!("detector:");
    for n in reg.detector_names() {
        println!("  {n}");
    }
}

fn dispatch(argv: &[String]) -> Result<(), String> {
    let Some(sub) = argv.first() else {
        return Err(USAGE.to_owned());
    };
    match sub.as_str() {
        "list" => {
            cmd_list();
            Ok(())
        }
        "check" | "run" => {
            let opts = parse_args(&argv[1..])?;
            let path = opts.config.ok_or("--config is required")?;
            if sub == "check" {
                cmd_check(&path)
            } else {
                cmd_run(&path, opts.duration_ms)
            }
        }
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            Ok(())
        }
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
