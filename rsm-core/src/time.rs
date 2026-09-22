//! 시각 타입과 시계 주입.
//!
//! 코어는 OS 시계를 **직접 호출하지 않는다**. 모든 시각 판정은 주입된
//! [`Clock`] 구현체를 통해 이뤄진다 (PRD D11 / TECH_STACK T6).
//!
//! 그 덕분에 얻는 것:
//!
//! - **리플레이 결정성** — 기록 파일의 타임스탬프를 [`VirtualClock`]에 넣으면
//!   재생 속도와 무관하게 같은 결과가 나온다 (PRD G5, R-10).
//! - **빠른 테스트** — "300 ms 후"를 실제로 기다리지 않고 시계를 전진시킨다.
//! - **시뮬레이션 시각** — `use_sim_time`이 켜진 ROS 2 환경에서는
//!   `rsm-ros2`의 `RosClock`이 `/clock`을 따라간다 (Phase 3).
//!
//! 벽시계(`SystemTime`)는 판정에 쓰지 않는다. NTP 보정으로 역행할 수 있어
//! TTL·스테일 판정이 깨지기 때문이다. 사람이 읽을 시각이 필요하면 Sink에서
//! "단조 시각 + 기동 시 오프셋"으로 변환한다.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// 단조 증가하는 시각.
///
/// 기준점(epoch)은 [`Clock`] 구현체가 정하며 그 자체로는 의미가 없다.
/// 코어가 요구하는 성질은 "단조 증가한다"와 "차이를 구할 수 있다" 둘뿐이다.
///
/// `u64` 나노초를 감싼 newtype이므로 런타임 비용은 없고, 길이·인덱스 같은
/// 다른 정수와 혼동하면 컴파일 에러가 난다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Instant(u64);

impl Instant {
    /// 기준점 그 자체(경과 0).
    pub const ZERO: Self = Self(0);

    /// 나노초 값으로부터 시각을 만든다. 리플레이·테스트에서 쓴다.
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// 기준점으로부터의 경과 나노초.
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// `earlier`로부터 경과한 시간. `earlier`가 더 나중이면 [`Duration::ZERO`].
    ///
    /// 뺄셈이 언더플로로 패닉하지 않도록 포화(saturating) 연산을 쓴다.
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }
}

/// 두 [`Instant`] 사이의 시간 간격.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Duration(u64);

impl Duration {
    /// 길이 0.
    pub const ZERO: Self = Self(0);

    /// 나노초로부터.
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    /// 마이크로초로부터.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros.saturating_mul(1_000))
    }
    /// 밀리초로부터.
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(1_000_000))
    }
    /// 초로부터.
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(1_000_000_000))
    }

    /// 나노초 값.
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    /// 밀리초 값(내림).
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }
}

impl std::ops::Add<Duration> for Instant {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl std::ops::Sub<Self> for Instant {
    type Output = Duration;
    /// 포화 뺄셈 — `rhs`가 더 나중이면 [`Duration::ZERO`].
    fn sub(self, rhs: Self) -> Duration {
        self.saturating_duration_since(rhs)
    }
}

impl std::ops::Add<Self> for Duration {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::Mul<u64> for Duration {
    type Output = Self;
    fn mul(self, rhs: u64) -> Self {
        Self(self.0.saturating_mul(rhs))
    }
}

/// 시각의 출처. 코어는 이 트레이트만 알고 구체 구현은 모른다.
///
/// `Send + Sync`인 이유는 주기 그룹 스레드 여럿이 하나의 시계를
/// `Arc<dyn Clock>`으로 공유해 동시에 읽기 때문이다.
///
/// # Examples
///
/// ```
/// use rsm_core::time::{Clock, Duration, Instant, VirtualClock};
///
/// fn is_stale(clock: &dyn Clock, last_input: Instant, threshold: Duration) -> bool {
///     clock.now() - last_input > threshold
/// }
///
/// let clock = VirtualClock::new();
/// let last = clock.now();
///
/// clock.advance(Duration::from_millis(299));
/// assert!(!is_stale(&clock, last, Duration::from_millis(300)));
///
/// clock.advance(Duration::from_millis(2));
/// assert!(is_stale(&clock, last, Duration::from_millis(300)));
/// ```
pub trait Clock: Send + Sync {
    /// 현재 시각.
    fn now(&self) -> Instant;
}

/// 실기·개발 PC용 단조 시계.
///
/// 표준 라이브러리의 단조 시계를 감싸며, 프로세스 안에서 이 타입만이
/// OS 시계를 직접 호출한다.
#[derive(Debug)]
pub struct MonotonicClock {
    epoch: std::time::Instant,
}

impl MonotonicClock {
    /// 지금을 기준점으로 삼아 시계를 만든다.
    #[allow(
        clippy::disallowed_methods,
        reason = "the only place in the core that reads the OS clock (PRD D11)"
    )]
    pub fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Instant {
        // u128 -> u64: 584년 이상 경과해야 포화한다.
        Instant::from_nanos(u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}

/// 리플레이·테스트용 가상 시계.
///
/// 시각을 OS에서 읽지 않고 내부 값에서 돌려준다. 그 값은 외부(리플레이
/// 드라이버나 테스트 코드)가 [`advance`](Self::advance) 또는
/// [`set`](Self::set)으로 전진시킨다.
///
/// 여러 스레드가 읽는 동안 드라이버가 쓰므로 원자 타입을 쓴다. `Release`/
/// `Acquire` 쌍이라 "시각을 본 시점 이후에는 그 전에 준비된 데이터도 보인다"가
/// 보장된다.
#[derive(Debug, Default)]
pub struct VirtualClock {
    nanos: AtomicU64,
}

impl VirtualClock {
    /// [`Instant::ZERO`]에서 시작하는 시계.
    pub fn new() -> Self {
        Self {
            nanos: AtomicU64::new(0),
        }
    }

    /// 지정한 시각에서 시작하는 시계.
    pub fn starting_at(start: Instant) -> Self {
        Self {
            nanos: AtomicU64::new(start.as_nanos()),
        }
    }

    /// 시계를 `delta`만큼 전진시킨다.
    pub fn advance(&self, delta: Duration) {
        self.nanos.fetch_add(delta.as_nanos(), Ordering::Release);
    }

    /// 시계를 특정 시각으로 맞춘다. 리플레이에서 레코드 타임스탬프를 따라갈 때 쓴다.
    ///
    /// 되돌리는 것도 가능하지만 단조성을 깨므로 리플레이 시작 시에만 쓸 것.
    pub fn set(&self, at: Instant) {
        self.nanos.store(at.as_nanos(), Ordering::Release);
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> Instant {
        Instant::from_nanos(self.nanos.load(Ordering::Acquire))
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use std::sync::Arc;

    use super::{Clock, Duration, Instant, MonotonicClock, VirtualClock};

    #[test]
    fn duration_conversions_are_consistent() {
        assert_eq!(Duration::from_millis(1).as_nanos(), 1_000_000);
        assert_eq!(Duration::from_secs(2).as_millis(), 2_000);
        assert_eq!(Duration::from_micros(1_500).as_nanos(), 1_500_000);
    }

    #[test]
    fn instant_subtraction_saturates_instead_of_panicking() {
        let early = Instant::from_nanos(100);
        let late = Instant::from_nanos(500);
        assert_eq!(late - early, Duration::from_nanos(400));
        assert_eq!(early - late, Duration::ZERO);
    }

    #[test]
    fn virtual_clock_advances_only_when_told() {
        let clock = VirtualClock::new();
        assert_eq!(clock.now(), Instant::ZERO);

        clock.advance(Duration::from_millis(250));
        assert_eq!(clock.now(), Instant::from_nanos(250_000_000));

        clock.set(Instant::from_nanos(42));
        assert_eq!(clock.now().as_nanos(), 42);
    }

    #[test]
    fn virtual_clock_is_shareable_across_threads() {
        let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new());
        let reader = Arc::clone(&clock);
        let handle = std::thread::spawn(move || reader.now());
        let seen = handle.join().unwrap_or(Instant::ZERO);
        assert!(seen >= Instant::ZERO);
    }

    #[test]
    fn monotonic_clock_never_goes_backwards() {
        let clock = MonotonicClock::new();
        let a = clock.now();
        let b = clock.now();
        assert!(b >= a);
    }
}
