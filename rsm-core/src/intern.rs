//! 문자열 인터닝 — 이름을 기동 시점에 한 번만 저장하고 런타임에는 정수로 다룬다.
//!
//! `fast` 그룹은 1 kHz로 돌면서 tick 경로에서 힙 할당이 0이어야 한다(R-08).
//! 그래서 [`HazardEvent`](crate::event::HazardEvent)에는 `String`을 넣을 수 없다.
//! 모듈 이름·위험 타입·근거 키·신호 이름을 설정 로드 시점에 이 표로 정수 ID로
//! 바꾸고, 문자열로 되돌리는 것은 Sink에서 출력할 때뿐이다.
//!
//! ```text
//!  설정 "internal.joint.torque_limit"  ──intern()──▶  id 7
//!                                                       │  런타임은 전부 정수
//!  JSONL "internal.joint.torque_limit" ◀───name()────────┘
//! ```

use std::collections::HashMap;

/// 인터닝 표 하나. 이름 ↔ 연속된 `u16` ID를 양방향으로 잇는다.
///
/// 등록은 기동 시점에만 일어난다. 런타임 조회([`name`](Self::name))는 읽기 전용이다.
#[derive(Debug, Default, Clone)]
pub struct Interner {
    names: Vec<String>,
    ids: HashMap<String, u16>,
}

/// 인터닝 표가 담을 수 있는 최대 항목 수. `u16` 범위를 넘지 않는다.
pub const MAX_INTERNED: usize = u16::MAX as usize;

impl Interner {
    /// 빈 표를 만든다.
    pub fn new() -> Self {
        Self::default()
    }

    /// 이름을 등록하고 ID를 돌려준다. 이미 있으면 기존 ID를 준다.
    ///
    /// 표가 가득 차면(`u16` 범위 초과) `None`. 설정 로드 시점에만 호출한다.
    pub fn intern(&mut self, name: &str) -> Option<u16> {
        if let Some(&id) = self.ids.get(name) {
            return Some(id);
        }
        if self.names.len() >= MAX_INTERNED {
            return None;
        }
        let id = u16::try_from(self.names.len()).ok()?;
        self.names.push(name.to_owned());
        self.ids.insert(name.to_owned(), id);
        Some(id)
    }

    /// 등록된 이름의 ID. 등록되지 않았으면 `None`.
    pub fn get(&self, name: &str) -> Option<u16> {
        self.ids.get(name).copied()
    }

    /// ID에 해당하는 이름. 출력 경로에서만 쓴다.
    pub fn name(&self, id: u16) -> Option<&str> {
        self.names.get(usize::from(id)).map(String::as_str)
    }

    /// 등록된 항목 수.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// 비어 있는지.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::Interner;

    #[test]
    fn same_name_yields_same_id() {
        let mut t = Interner::new();
        let a = t.intern("internal.joint.torque_limit");
        let b = t.intern("internal.joint.torque_limit");
        assert_eq!(a, b);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn round_trips_through_id() {
        let mut t = Interner::new();
        let id = t.intern("external.min_distance").unwrap_or(u16::MAX);
        assert_eq!(t.name(id), Some("external.min_distance"));
        assert_eq!(t.get("external.min_distance"), Some(id));
    }

    #[test]
    fn unknown_name_is_none() {
        let t = Interner::new();
        assert_eq!(t.get("no_such_name"), None);
        assert_eq!(t.name(0), None);
    }
}

/// 파이프라인이 쓰는 네 가지 이름 표를 한데 모은 것.
///
/// 기동 시점에 채워지고 그 뒤로는 읽기만 한다. Sink가 이벤트를 출력할 때
/// 정수 ID를 사람이 읽을 이름으로 되돌리는 데 쓴다.
#[derive(Debug, Default, Clone)]
pub struct Names {
    /// 모듈 인스턴스 이름.
    pub modules: Interner,
    /// 위험 타입 이름 — `internal.joint.torque_limit` 같은 계층 문자열.
    pub hazards: Interner,
    /// 근거 항목 키 — `measured`, `limit` 등.
    pub evidence: Interner,
    /// 신호 이름 — `joint.torque` 등.
    pub signals: Interner,
}

impl Names {
    /// 빈 이름 표.
    pub fn new() -> Self {
        Self::default()
    }
}
