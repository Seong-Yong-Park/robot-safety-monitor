//! 출력 Sink — JSON Lines (PRD R-09 "Sink: JSON Lines, stdout").
//!
//! 한 줄에 상태 스냅샷 하나. `jq`로 바로 걸러 볼 수 있다. 리플레이의 기대 결과(스냅샷
//! 시험)도 이 형식 그대로다 — 키는 이름순으로 나와 줄마다 모양이 같다.
//!
//! 여기가 **정수 ID가 다시 문자열이 되는 유일한 지점**이다. tick 경로는 끝까지
//! `u16`만 들고 다니고, 이름표([`Names`])는 출력 직전에만 펼친다 (R-08).

use std::io::{BufWriter, Write};

use rsm_core::event::SafetyState;
use rsm_core::intern::Names;
use rsm_core::traits::Sink;
use serde_json::{Map, Value, json};

/// 상태를 한 줄 JSON으로 적는 출력.
pub struct JsonlSink {
    out: BufWriter<Box<dyn Write + Send>>,
}

impl std::fmt::Debug for JsonlSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlSink").finish_non_exhaustive()
    }
}

impl JsonlSink {
    /// `path`가 `-` 면 표준출력, 아니면 그 파일에 덧붙여 쓴다.
    ///
    /// # Errors
    /// 파일을 열 수 없으면 [`std::io::Error`].
    pub fn open(path: &str) -> std::io::Result<Self> {
        let w: Box<dyn Write + Send> = if path == "-" {
            Box::new(std::io::stdout())
        } else {
            Box::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?,
            )
        };
        Ok(Self::to_writer(w))
    }

    /// 아무 출력에나 쓴다. 시험에서 결과를 메모리에 모을 때 쓴다.
    pub fn to_writer(w: Box<dyn Write + Send>) -> Self {
        Self {
            out: BufWriter::new(w),
        }
    }
}

fn name_of(interner: &rsm_core::intern::Interner, id: u16) -> &str {
    interner.name(id).unwrap_or("?")
}

impl Sink for JsonlSink {
    fn publish(&mut self, state: &SafetyState, names: &Names) {
        let active: Vec<Value> = state
            .active
            .iter()
            .map(|e| {
                let mut ev = Map::new();
                for (k, v) in e.evidence.iter() {
                    ev.insert(name_of(&names.evidence, k.0).to_owned(), json!(v));
                }
                json!({
                    "source": name_of(&names.modules, e.source.0),
                    "kind": name_of(&names.hazards, e.kind.0),
                    "category": e.category.as_str(),
                    "severity": e.severity.as_str(),
                    "confidence": e.confidence,
                    "observed_at_ns": e.observed_at.as_nanos(),
                    "ttl_ms": e.ttl.as_millis(),
                    "evidence": Value::Object(ev),
                })
            })
            .collect();

        let line = json!({
            "ts_ns": state.updated_at.as_nanos(),
            "level": state.level.as_str(),
            "availability": state.availability.as_str(),
            "recommended_action": state.recommended_action.as_str(),
            "active": active,
        });

        // 출력 실패로 감시기를 죽이지 않는다. 파이프가 닫힌 것뿐일 수 있다.
        drop(writeln!(self.out, "{line}"));
        drop(self.out.flush());
    }
}
