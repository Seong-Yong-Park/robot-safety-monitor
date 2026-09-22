//! 모듈 계약 (R-01).
//!
//! 네 개의 트레이트가 파이프라인의 교체 가능한 자리를 정의한다. 모듈은 이것만
//! 구현하면 되고 프레임워크 내부를 알 필요가 없다. 코어는 반대로 구체 타입을
//! 모른 채 `dyn` 으로만 다룬다.
//!
//! | 트레이트 | 자리 | 호출 시점 |
//! |---|---|---|
//! | [`Source`] | 입력 어댑터 | 매 tick 시작, Detector 보다 먼저 |
//! | [`Detector`] | 감지 단위 | 매 tick, 설정에 적힌 순서대로 |
//! | [`ArbiterPolicy`] | 융합 정책 | Arbiter 스레드에서 주기적으로 |
//! | [`Sink`] | 출력 어댑터 | 상태가 산출될 때마다 |

use crate::error::ConfigError;
use crate::event::{
    EvidenceKey, HazardEvent, HazardTypeId, ModuleId, MonitorAvailability, SafetyState,
};
use crate::intern::Names;
use crate::signal::{SignalBus, SignalId};
use crate::time::Instant;

/// 모듈이 스스로 보고하는 상태. Supervisor가 이것을 주기적으로 읽는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Health {
    /// 정상.
    #[default]
    Ok,
    /// 동작하지만 신뢰도가 떨어진다 — 입력이 오래됐다 등.
    Degraded,
    /// 고장. 패닉했거나 복구 불가능한 상태다.
    Faulted,
}

/// Detector가 이벤트를 내보내는 통로.
///
/// 구현체가 무엇인지(큐, 테스트용 버퍼) 모듈은 알 필요가 없다.
pub trait EventSink {
    /// 이벤트 하나를 발행한다. 큐가 가득 차면 구현체가 오버플로를 기록한다.
    fn emit(&mut self, event: HazardEvent);
}

/// 설정 단계에서 모듈에게 주어지는 문맥.
///
/// 파라미터를 읽고, 쓸 이름들을 인터닝해 ID를 받아 둔다. 런타임에는 여기서
/// 받은 ID만 쓰므로 tick 경로에 문자열이 남지 않는다.
pub struct ModuleCtx<'a> {
    id: ModuleId,
    name: String,
    params: &'a Params,
    names: &'a mut Names,
}

impl<'a> ModuleCtx<'a> {
    /// 새 문맥을 만든다. 설정 로더가 호출한다.
    pub fn new(id: ModuleId, name: String, params: &'a Params, names: &'a mut Names) -> Self {
        Self {
            id,
            name,
            params,
            names,
        }
    }

    /// 이 모듈의 ID. 발행하는 이벤트의 `source` 에 넣는다.
    pub const fn module_id(&self) -> ModuleId {
        self.id
    }

    /// 이 모듈의 인스턴스 이름. 오류 메시지에 쓴다.
    pub fn module_name(&self) -> &str {
        &self.name
    }

    /// 파라미터 접근자.
    pub const fn params(&self) -> &Params {
        self.params
    }

    /// 위험 타입 이름을 등록하고 ID를 받는다.
    ///
    /// # Errors
    /// 인터닝 표가 가득 찬 경우 [`ConfigError::TooManyNames`].
    pub fn hazard_type(&mut self, name: &str) -> Result<HazardTypeId, ConfigError> {
        self.names
            .hazards
            .intern(name)
            .map(HazardTypeId)
            .ok_or(ConfigError::TooManyNames)
    }

    /// 근거 키 이름을 등록하고 ID를 받는다.
    ///
    /// # Errors
    /// 인터닝 표가 가득 찬 경우 [`ConfigError::TooManyNames`].
    pub fn evidence_key(&mut self, name: &str) -> Result<EvidenceKey, ConfigError> {
        self.names
            .evidence
            .intern(name)
            .map(EvidenceKey)
            .ok_or(ConfigError::TooManyNames)
    }

    /// 신호 이름을 등록하고 ID를 받는다. Source의 출력과 Detector의 입력 모두 이걸 쓴다.
    ///
    /// # Errors
    /// 인터닝 표가 가득 찬 경우 [`ConfigError::TooManyNames`].
    pub fn signal(&mut self, name: &str) -> Result<SignalId, ConfigError> {
        self.names
            .signals
            .intern(name)
            .map(SignalId)
            .ok_or(ConfigError::TooManyNames)
    }
}

/// 모듈 파라미터. 설정 파일의 `params:` 매핑을 감싼 것.
#[derive(Debug, Clone, Default)]
pub struct Params {
    map: std::collections::BTreeMap<String, serde_yaml_ng::Value>,
    owner: String,
}

impl Params {
    /// YAML 매핑에서 만든다.
    pub fn new(
        owner: String,
        map: std::collections::BTreeMap<String, serde_yaml_ng::Value>,
    ) -> Self {
        Self { map, owner }
    }

    /// 실수 파라미터를 읽는다.
    ///
    /// # Errors
    /// 키가 없으면 [`ConfigError::MissingParam`], 타입이 다르면 [`ConfigError::BadParam`].
    pub fn f64(&self, key: &str) -> Result<f64, ConfigError> {
        let v = self.map.get(key).ok_or_else(|| ConfigError::MissingParam {
            module: self.owner.clone(),
            key: key.to_owned(),
        })?;
        v.as_f64().ok_or_else(|| ConfigError::BadParam {
            module: self.owner.clone(),
            key: key.to_owned(),
            expected: "a floating-point number",
        })
    }

    /// 실수 파라미터를 읽되 없으면 기본값을 쓴다.
    ///
    /// # Errors
    /// 키가 있는데 타입이 다르면 [`ConfigError::BadParam`].
    pub fn f64_or(&self, key: &str, default: f64) -> Result<f64, ConfigError> {
        if self.map.contains_key(key) {
            self.f64(key)
        } else {
            Ok(default)
        }
    }

    /// 문자열 파라미터를 읽는다.
    ///
    /// # Errors
    /// 키가 없으면 [`ConfigError::MissingParam`], 타입이 다르면 [`ConfigError::BadParam`].
    pub fn str(&self, key: &str) -> Result<&str, ConfigError> {
        let v = self.map.get(key).ok_or_else(|| ConfigError::MissingParam {
            module: self.owner.clone(),
            key: key.to_owned(),
        })?;
        v.as_str().ok_or_else(|| ConfigError::BadParam {
            module: self.owner.clone(),
            key: key.to_owned(),
            expected: "a string",
        })
    }

    /// 문자열 파라미터를 읽되 없으면 기본값을 쓴다.
    ///
    /// # Errors
    /// 키가 있는데 타입이 다르면 [`ConfigError::BadParam`].
    pub fn str_or<'s>(&'s self, key: &str, default: &'s str) -> Result<&'s str, ConfigError> {
        if self.map.contains_key(key) {
            self.str(key)
        } else {
            Ok(default)
        }
    }
}

/// 입력 어댑터. 외부 데이터를 신호 버스에 쓴다.
///
/// Phase 0에서는 Source가 Detector와 같은 그룹 스레드에서 돈다. 한 tick은
/// "모든 Source `poll` → 모든 Detector `tick`" 순서라 버스 접근에 락이 필요 없다.
pub trait Source: Send {
    /// 파라미터를 읽고 쓸 신호 ID를 확보한다.
    ///
    /// # Errors
    /// 파라미터가 없거나 타입이 다르면 [`ConfigError`].
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError>;

    /// 이 Source가 내보내는 신호 이름들. 설정 검증에서 연결을 확인하는 데 쓴다.
    fn declares(&self) -> Vec<String>;

    /// 한 tick 분량의 값을 버스에 쓴다.
    fn poll(&mut self, now: Instant, bus: &mut SignalBus);

    /// 현재 상태.
    fn health(&self) -> Health {
        Health::Ok
    }
}

/// 감지 단위. 위험 한 종류를 평가한다.
pub trait Detector: Send {
    /// 파라미터를 읽고 쓸 신호·타입 ID를 확보한다.
    ///
    /// # Errors
    /// 파라미터가 없거나 타입이 다르면 [`ConfigError`].
    fn configure(&mut self, ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError>;

    /// 이 Detector가 필요로 하는 신호 이름들. 하나라도 연결되지 않으면 기동 실패다.
    fn requires(&self) -> Vec<String>;

    /// 한 번 평가한다. 위험을 찾으면 `out` 으로 이벤트를 낸다.
    ///
    /// `now` 는 tick 시작 시점에 시계에서 한 번 읽은 값이다. 같은 tick의 모든
    /// 모듈이 같은 값을 보므로 판정이 실행 순서에 흔들리지 않는다.
    fn tick(&mut self, now: Instant, bus: &SignalBus, out: &mut dyn EventSink);

    /// 현재 상태.
    fn health(&self) -> Health {
        Health::Ok
    }
}

/// 이벤트를 종합 상태로 융합하는 정책 (R-05).
pub trait ArbiterPolicy: Send {
    /// 이벤트 하나를 받아들인다.
    fn ingest(&mut self, event: HazardEvent);

    /// 현재 시각 기준으로 종합 상태를 산출한다. 만료된 이벤트는 여기서 빠진다.
    fn evaluate(&mut self, now: Instant, availability: MonitorAvailability) -> SafetyState;
}

/// 출력 어댑터.
pub trait Sink: Send {
    /// 상태를 내보낸다. `names` 는 정수 ID를 사람이 읽을 이름으로 되돌리는 표다.
    fn publish(&mut self, state: &SafetyState, names: &Names);
}
