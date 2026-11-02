//! 요약 JSON 레코더 — v1의 계측 출력 (TECH_STACK T7).
//!
//! [`metrics`] 파사드의 레코더 구현이다. 값을 원자 변수에 들고 있다가, 실행이 끝나면
//! [`to_json`](SummaryRecorder::to_json)으로 한 번에 내보낸다. 실행 도중 수집하는 쪽
//! (Prometheus 등)은 P1(R-14)에서 다른 레코더로 붙인다 — 코어는 파사드만 알고 이
//! 크레이트를 모른다.
//!
//! ```json
//! [{"kind":"counter","labels":{"group":"fast"},"name":"rsm_group_ticks_total","value":2001}, ...]
//! ```
//!
//! 이름순·라벨순으로 정렬되어 나온다. 같은 실행이면 같은 순서다.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
use serde_json::{Value, json};

/// 레코더가 아는 계측 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
}

/// 이름 + 라벨(정렬). 맵의 키이자 출력 순서다.
type Id = (String, BTreeMap<String, String>);

#[derive(Debug, Default)]
struct Inner {
    entries: BTreeMap<Id, (Kind, Arc<AtomicU64>)>,
}

/// 요약 JSON 레코더. 복제본은 같은 값을 본다 — 하나는 설치하고 하나로 읽는다.
#[derive(Debug, Clone, Default)]
pub struct SummaryRecorder {
    inner: Arc<Mutex<Inner>>,
}

impl SummaryRecorder {
    /// 빈 레코더.
    pub fn new() -> Self {
        Self::default()
    }

    /// 이 레코더를 프로세스 전역 레코더로 설치한다. 실행 파일이 한 번 부른다.
    ///
    /// # Errors
    /// 이미 다른 레코더가 설치되어 있으면 오류 메시지.
    pub fn install_global(&self) -> Result<(), String> {
        metrics::set_global_recorder(self.clone())
            .map_err(|_| "a metrics recorder is already installed".to_owned())
    }

    /// 지금 값들. 카운터는 정수, 게이지는 실수다.
    pub fn snapshot(&self) -> Vec<(String, BTreeMap<String, String>, f64)> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner
            .entries
            .iter()
            .map(|((name, labels), (kind, cell))| (name.clone(), labels.clone(), read(*kind, cell)))
            .collect()
    }

    /// 지금 값들을 JSON 배열 한 줄로.
    pub fn to_json(&self) -> String {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let items: Vec<Value> = inner
            .entries
            .iter()
            .map(|((name, labels), (kind, cell))| {
                let value = match kind {
                    Kind::Counter => json!(cell.load(Ordering::Acquire)),
                    Kind::Gauge => json!(read(*kind, cell)),
                };
                json!({
                    "name": name,
                    "labels": labels,
                    "kind": match kind { Kind::Counter => "counter", Kind::Gauge => "gauge" },
                    "value": value,
                })
            })
            .collect();
        Value::Array(items).to_string()
    }

    fn cell(&self, key: &Key, kind: Kind) -> Arc<AtomicU64> {
        let id = (
            key.name().to_owned(),
            key.labels()
                .map(|l| (l.key().to_owned(), l.value().to_owned()))
                .collect(),
        );
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let (_, cell) = inner
            .entries
            .entry(id)
            .or_insert_with(|| (kind, Arc::new(AtomicU64::new(0))));
        Arc::clone(cell)
    }
}

/// 게이지는 `metrics`가 `AtomicU64`에 f64 비트로 담는다.
#[allow(
    clippy::cast_precision_loss,
    reason = "a counter shown as f64 in snapshot(); exact u64 is kept in to_json()"
)]
fn read(kind: Kind, cell: &AtomicU64) -> f64 {
    let raw = cell.load(Ordering::Acquire);
    match kind {
        Kind::Counter => raw as f64,
        Kind::Gauge => f64::from_bits(raw),
    }
}

impl Recorder for SummaryRecorder {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    // 등록은 시작할 때 한 번이다(MetricsExporter::new). 락은 여기서만 잡고, 값을 쓸 때는
    // 원자 변수 하나만 건드린다.
    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        Counter::from_arc(self.cell(key, Kind::Counter))
    }

    fn register_gauge(&self, key: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(self.cell(key, Kind::Gauge))
    }

    fn register_histogram(&self, _: &Key, _: &Metadata<'_>) -> Histogram {
        // 분포는 코어가 고정 칸 히스토그램으로 이미 백분위를 내서 게이지로 보낸다.
        Histogram::noop()
    }
}
