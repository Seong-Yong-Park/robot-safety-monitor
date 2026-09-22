//! # rsm-core
//!
//! Robot Safety Monitor의 미들웨어 독립 코어.
//!
//! 이 크레이트는 ROS 2, 특정 시뮬레이터, 특정 로봇 플랫폼을 **알지 못한다**.
//! 그런 결합은 전부 어댑터 크레이트(`rsm-ros2`, `rsm-mcap` 등)에 격리된다
//! (PRD D1). 의존성 방향이 그 규칙을 빌드 수준에서 강제한다.
//!
//! ## 한눈에 보는 흐름
//!
//! ```text
//!   YAML ──config──> Registry ──> [Source] ─Signal─> [Detector] ─HazardEvent─┐
//!                                      (그룹 스레드, SignalBus 공유)          │
//!                                                                    SPSC 큐 │
//!   [Supervisor] ─HealthRegistry(원자변수)─> [Arbiter] <───────────────────────┘
//!                                               │
//!                                          SafetyState ──> [Sink] ──> JSONL
//! ```
//!
//! 문자열은 **입구(YAML)와 출구(JSONL)에만** 있다. 그 사이의 tick 경로는
//! 인터닝된 `u16` ID만 들고 다니므로 힙 할당이 없다 (R-08).
//!
//! ## 구성
//!
//! | 모듈 | 역할 | 먼저 읽을 것 |
//! |---|---|---|
//! | [`time`] | 시각 타입과 [`Clock`](time::Clock) 주입 (D7) | [`Clock`](time::Clock) |
//! | [`event`] | `HazardEvent` / `SafetyState` 스키마 (R-04) | [`HazardEvent`](event::HazardEvent) |
//! | [`signal`] | Source→Detector 신호와 버스 | [`SignalBus`](signal::SignalBus) |
//! | [`intern`] | 이름 → 정수 ID 표 (R-08) | [`Interner`](intern::Interner) |
//! | [`traits`] | `Source` / `Detector` / `ArbiterPolicy` / `Sink` (R-01) | [`Detector`](traits::Detector) |
//! | [`registry`] | 이름 → 팩토리 등록 (R-03) | [`Registry`](registry::Registry) |
//! | [`config`] | YAML 선언 로드·검증·조립 (R-02) | [`PipelineConfig::build`](config::PipelineConfig::build) |
//! | [`schedule`] | 주기 그룹·Arbiter·Supervisor 스레드 (R-03, R-18) | [`run`](schedule::run) |
//! | [`arbiter`] | max-severity + TTL 융합 정책 (R-05) | [`MaxSeverity`](arbiter::MaxSeverity) |
//! | [`supervisor`] | 모듈·그룹 건강 감시, 가용성 판정 (R-06, R-16) | [`HealthRegistry`](supervisor::HealthRegistry) |
//! | [`error`] | 설정·기동 오류 | [`ConfigError`](error::ConfigError) |

pub mod arbiter;
pub mod config;
pub mod error;
pub mod event;
pub mod intern;
pub mod registry;
pub mod schedule;
pub mod signal;
pub mod supervisor;
pub mod time;
pub mod traits;
