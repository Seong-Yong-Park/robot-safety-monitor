//! 이름 → 팩토리 등록표 (R-03).
//!
//! 모듈은 이름으로 등록되고 설정 파일에서 이름으로 참조된다. 새 모듈을 추가할 때
//! `rsm-core` 는 한 줄도 바뀌지 않는다(G2) — 모듈 크레이트의 등록 함수만 는다.
//!
//! 팩토리 시그니처를 `fn` 포인터로 고정한 이유는 P2의 동적 플러그인(R-19) 때문이다.
//! 그때가 오면 팩토리의 출처만 정적 링크에서 `dlsym` 으로 바뀌고 나머지는 그대로다.

use std::collections::BTreeMap;

use crate::traits::{Detector, Source};

/// Source를 하나 만드는 함수.
pub type SourceFactory = fn() -> Box<dyn Source>;

/// Detector를 하나 만드는 함수.
pub type DetectorFactory = fn() -> Box<dyn Detector>;

/// 설정에서 참조할 수 있는 모듈들의 목록.
#[derive(Default)]
pub struct Registry {
    sources: BTreeMap<String, SourceFactory>,
    detectors: BTreeMap<String, DetectorFactory>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("sources", &self.sources.keys().collect::<Vec<_>>())
            .field("detectors", &self.detectors.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Registry {
    /// 빈 등록표.
    pub fn new() -> Self {
        Self::default()
    }

    /// Source를 등록한다. 같은 이름이 이미 있으면 덮어쓴다.
    pub fn register_source(&mut self, name: &str, factory: SourceFactory) -> &mut Self {
        self.sources.insert(name.to_owned(), factory);
        self
    }

    /// Detector를 등록한다. 같은 이름이 이미 있으면 덮어쓴다.
    pub fn register_detector(&mut self, name: &str, factory: DetectorFactory) -> &mut Self {
        self.detectors.insert(name.to_owned(), factory);
        self
    }

    /// 등록된 Source를 하나 만든다.
    pub fn make_source(&self, name: &str) -> Option<Box<dyn Source>> {
        self.sources.get(name).map(|f| f())
    }

    /// 등록된 Detector를 하나 만든다.
    pub fn make_detector(&self, name: &str) -> Option<Box<dyn Detector>> {
        self.detectors.get(name).map(|f| f())
    }

    /// 등록된 Source 이름들. 오류 메시지에서 "혹시 이것을 쓰려던 것인가" 를 보일 때 쓴다.
    pub fn source_names(&self) -> impl Iterator<Item = &str> {
        self.sources.keys().map(String::as_str)
    }

    /// 등록된 Detector 이름들.
    pub fn detector_names(&self) -> impl Iterator<Item = &str> {
        self.detectors.keys().map(String::as_str)
    }
}
