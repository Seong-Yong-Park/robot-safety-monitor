//! YAML 선언을 읽고 파이프라인을 조립한다 (R-02).
//!
//! 이 단계에서 모든 동적 결정과 대부분의 할당이 끝난다. 그 덕분에 이후 tick
//! 경로에 할당이 없다는 요구(R-08)가 성립한다.
//!
//! 순서는 이렇다. YAML 파싱 → 레지스트리 조회 → 이름 인터닝 → 팩토리 호출과
//! `configure` → 신호 연결 검증 → 그룹 배치. **어느 단계에서든 모호하면 기동
//! 실패다.** 묵시적으로 무시하고 돌면, 감시한다고 믿는 것이 사실은 감시되지
//! 않는 상태가 된다.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::error::ConfigError;
use crate::event::ModuleId;
use crate::intern::Names;
use crate::registry::Registry;
use crate::time::Duration;
use crate::traits::{Detector, ModuleCtx, Params, Source};

/// 설정 파일의 최상위 구조.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineConfig {
    /// 스키마 버전. 현재 1만 지원한다.
    pub version: u32,
    /// 주기 그룹 정의. 이름 → 설정.
    pub groups: BTreeMap<String, GroupConfig>,
    /// 모듈 인스턴스 목록.
    pub modules: Vec<ModuleConfig>,
    /// Arbiter 설정.
    #[serde(default)]
    pub arbiter: ArbiterConfig,
    /// Supervisor 설정.
    #[serde(default)]
    pub supervisor: SupervisorConfig,
    /// 출력 설정.
    #[serde(default)]
    pub sink: SinkConfig,
}

/// 주기 그룹 하나.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupConfig {
    /// 실행 주기(Hz).
    pub period_hz: u32,
    /// 이 그룹에서 Arbiter로 가는 큐의 용량.
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,
}

const fn default_queue_capacity() -> usize {
    256
}

/// 모듈 인스턴스 하나.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleConfig {
    /// 이 인스턴스의 이름. 파이프라인 안에서 유일해야 한다.
    pub name: String,
    /// 레지스트리에 등록된 모듈 종류 이름.
    #[serde(rename = "type")]
    pub kind: String,
    /// 배치될 주기 그룹.
    pub group: String,
    /// 모듈별 파라미터.
    #[serde(default)]
    pub params: BTreeMap<String, serde_yaml_ng::Value>,
}

/// Arbiter 설정.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbiterConfig {
    /// 상태를 산출하고 내보내는 주기(Hz).
    #[serde(default = "default_arbiter_hz")]
    pub period_hz: u32,
}

const fn default_arbiter_hz() -> u32 {
    20
}

impl Default for ArbiterConfig {
    fn default() -> Self {
        Self {
            period_hz: default_arbiter_hz(),
        }
    }
}

/// Supervisor 설정.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    /// 건강 상태를 훑는 주기(Hz).
    #[serde(default = "default_supervisor_hz")]
    pub period_hz: u32,
    /// 그룹이 이 시간 넘게 tick 하지 않으면 감시 불가로 본다(ms).
    #[serde(default = "default_stale_ms")]
    pub group_stale_after_ms: u64,
}

const fn default_supervisor_hz() -> u32 {
    10
}
const fn default_stale_ms() -> u64 {
    500
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            period_hz: default_supervisor_hz(),
            group_stale_after_ms: default_stale_ms(),
        }
    }
}

/// 출력 설정.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    /// 출력 형식. 현재 `jsonl` 만 있다.
    #[serde(default = "default_sink_format")]
    pub format: String,
    /// 출력 대상. `-` 는 표준 출력.
    #[serde(default = "default_sink_path")]
    pub path: String,
    /// 상태가 바뀌었을 때만 내보낼지.
    #[serde(default)]
    pub on_change_only: bool,
}

fn default_sink_format() -> String {
    "jsonl".to_owned()
}
fn default_sink_path() -> String {
    "-".to_owned()
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            format: default_sink_format(),
            path: default_sink_path(),
            on_change_only: false,
        }
    }
}

/// 한 그룹에 배치된 모듈들 — `(Source 목록, Detector 목록)`.
type GroupModules = (
    Vec<(ModuleId, Box<dyn Source>)>,
    Vec<(ModuleId, Box<dyn Detector>)>,
);

/// 신호 연결 검증에 쓰는 중간 결과.
struct Wiring {
    /// `(그룹, 신호)` → 그 신호를 내는 모듈.
    declared: BTreeMap<(String, String), String>,
    /// `(그룹, 신호, 그 신호를 요구한 모듈)`.
    required: Vec<(String, String, String)>,
}

/// 조립된 그룹 하나. 스레드 하나가 이것을 통째로 소유한다.
pub struct GroupPlan {
    /// 그룹 이름.
    pub name: String,
    /// 실행 주기.
    pub period: Duration,
    /// 큐 용량.
    pub queue_capacity: usize,
    /// 이 그룹의 Source들. tick 마다 먼저 실행된다.
    pub sources: Vec<(ModuleId, Box<dyn Source>)>,
    /// 이 그룹의 Detector들. 설정에 적힌 순서대로 실행된다.
    pub detectors: Vec<(ModuleId, Box<dyn Detector>)>,
}

impl std::fmt::Debug for GroupPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupPlan")
            .field("name", &self.name)
            .field("period", &self.period)
            .field("sources", &self.sources.len())
            .field("detectors", &self.detectors.len())
            .finish_non_exhaustive()
    }
}

/// 조립 결과. 스케줄러가 이걸 받아 스레드를 띄운다.
#[derive(Debug)]
pub struct BuiltPipeline {
    /// 그룹들. 설정의 이름 순.
    pub groups: Vec<GroupPlan>,
    /// 인터닝된 이름 표.
    pub names: Names,
    /// 전체 신호 개수. 각 그룹 버스의 크기가 된다.
    pub signal_count: usize,
    /// 모듈 총 개수. 건강 표의 크기가 된다.
    pub module_count: usize,
    /// Arbiter 주기.
    pub arbiter_period: Duration,
    /// Supervisor 주기.
    pub supervisor_period: Duration,
    /// 그룹 스테일 판정 임계.
    pub group_stale_after: Duration,
    /// 출력 설정.
    pub sink: SinkConfig,
}

const fn hz_to_period(hz: u32) -> Duration {
    if hz == 0 {
        Duration::from_millis(100)
    } else {
        Duration::from_nanos(1_000_000_000 / hz as u64)
    }
}

impl PipelineConfig {
    /// 파일에서 읽는다.
    ///
    /// # Errors
    /// 파일을 못 열거나 YAML이 스키마와 맞지 않으면 [`ConfigError`].
    pub fn from_path(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_str(&text)
    }

    /// 문자열에서 읽는다.
    ///
    /// # Errors
    /// YAML이 스키마와 맞지 않으면 [`ConfigError::Yaml`].
    #[allow(
        clippy::should_implement_trait,
        reason = "avoids the Err bound imposed by the FromStr trait"
    )]
    pub fn from_str(text: &str) -> Result<Self, ConfigError> {
        let cfg: Self = serde_yaml_ng::from_str(text)?;
        Ok(cfg)
    }

    /// 레지스트리를 보고 실제 모듈을 만들어 조립한다.
    ///
    /// # Errors
    /// 미지원 버전, 빈 그룹, 미등록 모듈, 중복 이름, 미정의 그룹, 미연결 입력,
    /// 신호 중복 선언 중 하나라도 있으면 [`ConfigError`].
    pub fn build(self, registry: &Registry) -> Result<BuiltPipeline, ConfigError> {
        self.check_header()?;

        let mut names = Names::new();
        let mut per_group: BTreeMap<String, GroupModules> = BTreeMap::new();
        let wiring = self.instantiate(registry, &mut names, &mut per_group)?;

        for (group, signal, module) in wiring.required {
            if !wiring.declared.contains_key(&(group, signal.clone())) {
                return Err(ConfigError::UnconnectedInput { module, signal });
            }
        }

        let mut groups = Vec::new();
        for (gname, gcfg) in &self.groups {
            let (sources, detectors) = per_group.remove(gname).unwrap_or_default();
            if sources.is_empty() && detectors.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "group `{gname}` has no modules assigned"
                )));
            }
            groups.push(GroupPlan {
                name: gname.clone(),
                period: hz_to_period(gcfg.period_hz),
                queue_capacity: gcfg.queue_capacity.max(1),
                sources,
                detectors,
            });
        }

        Ok(BuiltPipeline {
            signal_count: names.signals.len(),
            module_count: names.modules.len(),
            groups,
            names,
            arbiter_period: hz_to_period(self.arbiter.period_hz),
            supervisor_period: hz_to_period(self.supervisor.period_hz),
            group_stale_after: Duration::from_millis(self.supervisor.group_stale_after_ms),
            sink: self.sink,
        })
    }

    /// 버전·빈 목록 같은 최상위 전제를 확인한다.
    fn check_header(&self) -> Result<(), ConfigError> {
        if self.version != 1 {
            return Err(ConfigError::Invalid(format!(
                "unsupported config version {} (only 1 is supported)",
                self.version
            )));
        }
        if self.groups.is_empty() {
            return Err(ConfigError::Invalid(
                "no periodic groups defined".to_owned(),
            ));
        }
        if self.modules.is_empty() {
            return Err(ConfigError::Invalid("no modules defined".to_owned()));
        }
        Ok(())
    }

    /// 모듈을 만들고 `configure` 한 뒤 그룹별로 나눠 담는다.
    fn instantiate(
        &self,
        registry: &Registry,
        names: &mut Names,
        per_group: &mut BTreeMap<String, GroupModules>,
    ) -> Result<Wiring, ConfigError> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut wiring = Wiring {
            declared: BTreeMap::new(),
            required: Vec::new(),
        };

        for m in &self.modules {
            if !seen.insert(m.name.as_str()) {
                return Err(ConfigError::DuplicateModule {
                    name: m.name.clone(),
                });
            }
            if !self.groups.contains_key(&m.group) {
                return Err(ConfigError::UnknownGroup {
                    module: m.name.clone(),
                    group: m.group.clone(),
                });
            }

            let id = ModuleId(
                names
                    .modules
                    .intern(&m.name)
                    .ok_or(ConfigError::TooManyNames)?,
            );
            let params = Params::new(m.name.clone(), m.params.clone());
            let entry = per_group.entry(m.group.clone()).or_default();

            if let Some(mut src) = registry.make_source(&m.kind) {
                {
                    let mut ctx = ModuleCtx::new(id, m.name.clone(), &params, names);
                    src.configure(&mut ctx)?;
                }
                for sig in src.declares() {
                    let key = (m.group.clone(), sig.clone());
                    if let Some(first) = wiring.declared.get(&key) {
                        return Err(ConfigError::DuplicateSignal {
                            signal: sig,
                            first: first.clone(),
                            second: m.name.clone(),
                        });
                    }
                    wiring.declared.insert(key, m.name.clone());
                    names
                        .signals
                        .intern(&sig)
                        .ok_or(ConfigError::TooManyNames)?;
                }
                entry.0.push((id, src));
            } else if let Some(mut det) = registry.make_detector(&m.kind) {
                {
                    let mut ctx = ModuleCtx::new(id, m.name.clone(), &params, names);
                    det.configure(&mut ctx)?;
                }
                for sig in det.requires() {
                    wiring.required.push((m.group.clone(), sig, m.name.clone()));
                }
                entry.1.push((id, det));
            } else {
                return Err(ConfigError::UnknownModule {
                    kind: "source/detector",
                    name: m.kind.clone(),
                });
            }
        }
        Ok(wiring)
    }
}
