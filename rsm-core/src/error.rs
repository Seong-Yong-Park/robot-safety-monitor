//! 설정·기동 단계의 오류 (R-02).
//!
//! 모호한 설정은 **기동 실패**다. 묵시적으로 무시하고 돌기 시작하면, 감시하고
//! 있다고 믿는 것이 사실은 감시되지 않는 상태가 된다.

use thiserror::Error;

/// 설정을 읽고 파이프라인을 조립하는 동안 생길 수 있는 오류.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// YAML 문법 오류 또는 스키마 불일치.
    #[error("cannot parse the configuration file")]
    Yaml(#[from] serde_yaml_ng::Error),

    /// 설정 파일 자체를 열 수 없다.
    #[error("configuration file I/O error")]
    Io(#[from] std::io::Error),

    /// 레지스트리에 없는 모듈을 설정이 참조했다.
    #[error("unregistered {kind} module: `{name}`")]
    UnknownModule {
        /// `source` 또는 `detector`.
        kind: &'static str,
        /// 설정에 적힌 이름.
        name: String,
    },

    /// 모듈이 요구하는 입력 신호를 아무 Source도 내지 않는다.
    #[error("no Source declares signal `{signal}` required by module `{module}`")]
    UnconnectedInput {
        /// 입력을 요구한 모듈.
        module: String,
        /// 연결되지 않은 신호 이름.
        signal: String,
    },

    /// 같은 신호를 두 Source가 낸다. 누가 이기는지 정할 수 없다.
    #[error("signal `{signal}` is declared by both `{first}` and `{second}`")]
    DuplicateSignal {
        /// 충돌한 신호 이름.
        signal: String,
        /// 먼저 선언한 모듈.
        first: String,
        /// 나중에 선언한 모듈.
        second: String,
    },

    /// 설정이 존재하지 않는 주기 그룹을 가리킨다.
    #[error("module `{module}` is placed in undefined group `{group}`")]
    UnknownGroup {
        /// 배치된 모듈.
        module: String,
        /// 설정에 적힌 그룹 이름.
        group: String,
    },

    /// 모듈 인스턴스 이름이 중복된다.
    #[error("duplicate module instance name: `{name}`")]
    DuplicateModule {
        /// 중복된 이름.
        name: String,
    },

    /// 필수 파라미터가 없다.
    #[error("module `{module}` is missing required parameter `{key}`")]
    MissingParam {
        /// 파라미터를 요구한 모듈.
        module: String,
        /// 빠진 키.
        key: String,
    },

    /// 파라미터 타입이 기대와 다르다.
    #[error("module `{module}`: parameter `{key}` must be {expected}")]
    BadParam {
        /// 문제의 모듈.
        module: String,
        /// 문제의 키.
        key: String,
        /// 기대한 타입.
        expected: &'static str,
    },

    /// 설정 내용이 논리적으로 성립하지 않는다.
    #[error("invalid configuration: {0}")]
    Invalid(String),

    /// 인터닝 표가 가득 찼다(이름 65,535개 초과).
    #[error("too many names — the interning table is full")]
    TooManyNames,
}
