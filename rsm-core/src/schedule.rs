//! 주기 실행 — 그룹 스레드·Arbiter 스레드·Supervisor 스레드 (R-03, R-16, R-18).
//!
//! # 구조
//!
//! ```text
//!   [group "fast"]  --SPSC-->\
//!   [group "normal"] --SPSC-->  [arbiter] --> [sink]
//!                               ^
//!   [supervisor] --HealthRegistry(atomics)--/
//! ```
//!
//! 그룹 스레드는 각자 자기 [`SignalBus`]를 통째로 소유한다. 한 tick은
//! "모든 Source `poll` → 모든 Detector `tick`" 순서라 버스에 락이 필요 없다.
//! 그룹 밖으로 나가는 것은 [`HazardEvent`] 뿐이고, 그것은 wait-free SPSC 링버퍼로
//! Arbiter에게 간다. Supervisor는 큐를 쓰지 않고 원자 변수 표
//! ([`HealthRegistry`])만 읽고 쓴다.
//!
//! # 시각
//!
//! 한 tick의 모든 모듈은 tick 시작 때 시계에서 한 번 읽은 같은 `now`를 본다.
//! 그래야 판정이 모듈 실행 순서에 흔들리지 않는다.
//!
//! # 패닉 격리
//!
//! 모듈 하나가 패닉해도 감시기 전체가 죽으면 안 된다(R-18). 각 모듈 호출은
//! [`catch_unwind`]로 감싸고, 패닉한 모듈은 `Faulted`로 표시한 뒤 이후 tick에서
//! 건너뛴다. 감시 범위가 줄었다는 사실은 Supervisor가 `Degraded`로 알린다.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::config::{BuiltPipeline, GroupPlan};
use crate::event::{HazardEvent, MonitorAvailability, Severity};
use crate::intern::Names;
use crate::signal::SignalBus;
use crate::supervisor::HealthRegistry;
use crate::time::{Clock, Duration, Instant};
use crate::traits::{ArbiterPolicy, EventSink, Health, Sink};

/// 그룹 스레드가 이벤트를 내보내는 통로. 링버퍼 생산자를 감싼다.
///
/// 큐가 가득 차면 이벤트를 **버리고** 오버플로를 기록한다. 여기서 블로킹하면
/// 1 kHz 그룹이 Arbiter 속도에 묶이므로, 주기를 지키는 쪽을 택하고 대신
/// "빠뜨린 게 있다"는 사실을 Supervisor가 `Degraded`로 알린다.
pub struct QueueSink {
    tx: Producer<HazardEvent>,
    group: usize,
    health: Arc<HealthRegistry>,
}

impl QueueSink {
    /// 링버퍼 생산자를 감싼다.
    pub fn new(tx: Producer<HazardEvent>, group: usize, health: Arc<HealthRegistry>) -> Self {
        Self { tx, group, health }
    }
}

impl EventSink for QueueSink {
    fn emit(&mut self, event: HazardEvent) {
        if self.tx.push(event).is_err() {
            self.health.record_overflow(self.group);
        }
    }
}

/// 테스트용 이벤트 수집기. 모듈을 스레드 없이 단독으로 돌려 볼 때 쓴다.
#[derive(Debug, Default)]
pub struct VecSink {
    /// 지금까지 받은 이벤트.
    pub events: Vec<HazardEvent>,
}

impl VecSink {
    /// 빈 수집기.
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventSink for VecSink {
    fn emit(&mut self, event: HazardEvent) {
        self.events.push(event);
    }
}

/// 주기 그룹 하나를 도는 일꾼. 스레드 하나가 이것을 통째로 소유한다.
///
/// [`tick`](Self::tick)은 스레드·수면과 무관한 순수 함수에 가까워서,
/// [`VirtualClock`](crate::time::VirtualClock)으로 한 tick씩 밟으며 테스트할 수 있다.
pub struct GroupWorker {
    plan: GroupPlan,
    bus: SignalBus,
    index: usize,
    /// Source 먼저, 그다음 Detector 순으로 늘어놓은 고장 표.
    /// tick 경로에서 할당하지 않으려고 기동 때 한 번만 잡는다.
    faulted: Vec<bool>,
}

impl std::fmt::Debug for GroupWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupWorker")
            .field("name", &self.plan.name)
            .field("period", &self.plan.period)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl GroupWorker {
    /// 그룹 계획을 받아 일꾼을 만든다. 버스와 고장 표의 할당은 여기서 끝난다.
    pub fn new(plan: GroupPlan, signal_count: usize, index: usize) -> Self {
        let n = plan.sources.len() + plan.detectors.len();
        Self {
            bus: SignalBus::with_capacity(signal_count),
            faulted: vec![false; n],
            plan,
            index,
        }
    }

    /// 그룹 이름.
    pub fn name(&self) -> &str {
        &self.plan.name
    }

    /// 실행 주기.
    pub fn period(&self) -> Duration {
        self.plan.period
    }

    /// 이 그룹의 버스(검사용).
    pub fn bus(&self) -> &SignalBus {
        &self.bus
    }

    /// 한 tick. Source를 모두 돌린 뒤 Detector를 설정 순서대로 돌린다.
    pub fn tick(&mut self, now: Instant, health: &HealthRegistry, out: &mut dyn EventSink) {
        let src_count = self.plan.sources.len();

        for (slot, (id, source)) in self.plan.sources.iter_mut().enumerate() {
            if self.faulted[slot] {
                continue;
            }
            let bus = &mut self.bus;
            let r = catch_unwind(AssertUnwindSafe(|| source.poll(now, bus)));
            if r.is_err() {
                self.faulted[slot] = true;
                health.record_fault(*id);
                health.set_module_health(*id, Health::Faulted);
                continue;
            }
            health.record_module_tick(*id, now);
            health.set_module_health(*id, source.health());
        }

        for (i, (id, detector)) in self.plan.detectors.iter_mut().enumerate() {
            let slot = src_count + i;
            if self.faulted[slot] {
                continue;
            }
            let bus = &self.bus;
            let r = catch_unwind(AssertUnwindSafe(|| detector.tick(now, bus, out)));
            if r.is_err() {
                self.faulted[slot] = true;
                health.record_fault(*id);
                health.set_module_health(*id, Health::Faulted);
                continue;
            }
            health.record_module_tick(*id, now);
            health.set_module_health(*id, detector.health());
        }
    }
}

/// 이벤트를 모아 종합 상태를 만들고 출력까지 보내는 일꾼.
pub struct ArbiterWorker {
    policy: Box<dyn ArbiterPolicy>,
    consumers: Vec<Consumer<HazardEvent>>,
    sink: Box<dyn Sink>,
    names: Arc<Names>,
    on_change_only: bool,
    last: Option<(Severity, MonitorAvailability, usize)>,
}

impl std::fmt::Debug for ArbiterWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArbiterWorker")
            .field("inputs", &self.consumers.len())
            .field("on_change_only", &self.on_change_only)
            .finish_non_exhaustive()
    }
}

impl ArbiterWorker {
    /// 정책·입력 큐·출력을 묶는다.
    pub fn new(
        policy: Box<dyn ArbiterPolicy>,
        consumers: Vec<Consumer<HazardEvent>>,
        sink: Box<dyn Sink>,
        names: Arc<Names>,
        on_change_only: bool,
    ) -> Self {
        Self {
            policy,
            consumers,
            sink,
            names,
            on_change_only,
            last: None,
        }
    }

    /// 한 tick. 큐를 모두 비우고, 평가하고, 필요하면 출력한다.
    pub fn tick(&mut self, now: Instant, availability: MonitorAvailability) {
        for c in &mut self.consumers {
            while let Ok(ev) = c.pop() {
                self.policy.ingest(ev);
            }
        }

        let state = self.policy.evaluate(now, availability);
        let key = (state.level, state.availability, state.active.len());

        // `on_change_only`가 false면 매 주기 내보낸다. 이것이 곧 하트비트다 —
        // 소비자는 "조용함"을 안전으로 읽으면 안 되고, 하트비트가 끊긴 것으로
        // 감시기 자체의 이상을 알아챈다(R-16).
        if !self.on_change_only || self.last.as_ref() != Some(&key) {
            self.sink.publish(&state, &self.names);
            self.last = Some(key);
        }
    }
}

/// 돌고 있는 파이프라인. 스레드 핸들과 정지 플래그를 쥔다.
#[derive(Debug)]
pub struct Runtime {
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    health: Arc<HealthRegistry>,
}

impl Runtime {
    /// 모든 스레드에게 멈추라고 알린다. 실제 종료는 각자 다음 tick 경계에서 난다.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// 정지 플래그. Ctrl-C 핸들러 등에 넘겨 쓴다.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// 건강 표. 통계를 읽는 데 쓴다.
    pub fn health(&self) -> &Arc<HealthRegistry> {
        &self.health
    }

    /// 모든 스레드가 끝날 때까지 기다린다.
    pub fn join(self) {
        for h in self.handles {
            drop(h.join());
        }
    }
}

/// 절대 데드라인 기준으로 잰다. 한 tick이 늦어도 다음 데드라인이 밀리지 않는다.
fn sleep_until(clock: &dyn Clock, deadline: Instant) {
    let now = clock.now();
    if deadline > now {
        let ns = (deadline - now).as_nanos();
        thread::sleep(std::time::Duration::from_nanos(ns));
    }
}

/// 조립된 파이프라인을 실제로 돌린다.
///
/// 그룹마다 스레드 하나, Arbiter 하나, Supervisor 하나를 띄우고 즉시 반환한다.
/// 멈추려면 [`Runtime::stop`] 뒤에 [`Runtime::join`]을 부른다.
pub fn run(
    pipeline: BuiltPipeline,
    clock: &Arc<dyn Clock>,
    policy: Box<dyn ArbiterPolicy>,
    sink: Box<dyn Sink>,
) -> Runtime {
    let BuiltPipeline {
        groups,
        names,
        signal_count,
        module_count,
        arbiter_period,
        supervisor_period,
        group_stale_after,
        sink: sink_cfg,
    } = pipeline;

    let health = Arc::new(HealthRegistry::new(module_count, groups.len()));
    let names = Arc::new(names);
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(groups.len() + 2);
    let mut consumers = Vec::with_capacity(groups.len());

    for (index, plan) in groups.into_iter().enumerate() {
        let capacity = plan.queue_capacity;
        let (tx, rx) = RingBuffer::<HazardEvent>::new(capacity);
        consumers.push(rx);

        let worker = GroupWorker::new(plan, signal_count, index);
        let out = QueueSink::new(tx, index, Arc::clone(&health));
        let clock = Arc::clone(clock);
        let health = Arc::clone(&health);
        let stop = Arc::clone(&stop);
        let name = format!("rsm-group-{}", worker.name());

        let h = thread::Builder::new()
            .name(name)
            .spawn(move || group_loop(worker, index, &*clock, &health, out, &stop));
        if let Ok(h) = h {
            handles.push(h);
        }
    }

    {
        let mut arbiter = ArbiterWorker::new(
            policy,
            consumers,
            sink,
            Arc::clone(&names),
            sink_cfg.on_change_only,
        );
        let clock = Arc::clone(clock);
        let health = Arc::clone(&health);
        let stop = Arc::clone(&stop);
        let h = thread::Builder::new()
            .name("rsm-arbiter".to_owned())
            .spawn(move || {
                let mut deadline = clock.now();
                while !stop.load(Ordering::Relaxed) {
                    arbiter.tick(clock.now(), health.availability());
                    deadline += arbiter_period;
                    sleep_until(&*clock, deadline);
                    let now = clock.now();
                    if deadline < now {
                        deadline = now;
                    }
                }
            });
        if let Ok(h) = h {
            handles.push(h);
        }
    }

    {
        let clock = Arc::clone(clock);
        let health = Arc::clone(&health);
        let stop = Arc::clone(&stop);
        let h = thread::Builder::new()
            .name("rsm-supervisor".to_owned())
            .spawn(move || {
                let mut deadline = clock.now();
                while !stop.load(Ordering::Relaxed) {
                    health.scan(clock.now(), group_stale_after);
                    deadline += supervisor_period;
                    sleep_until(&*clock, deadline);
                    let now = clock.now();
                    if deadline < now {
                        deadline = now;
                    }
                }
            });
        if let Ok(h) = h {
            handles.push(h);
        }
    }

    Runtime {
        stop,
        handles,
        health,
    }
}

fn group_loop(
    mut worker: GroupWorker,
    index: usize,
    clock: &dyn Clock,
    health: &HealthRegistry,
    mut out: QueueSink,
    stop: &AtomicBool,
) {
    let period = worker.period();
    let mut deadline = clock.now();

    while !stop.load(Ordering::Relaxed) {
        let started = clock.now();
        worker.tick(started, health, &mut out);
        let ended = clock.now();
        let elapsed = ended - started;
        health.record_group_tick(index, started, elapsed, elapsed > period);

        deadline += period;
        sleep_until(clock, deadline);

        // 데드라인을 이미 지나쳤으면 밀린 주기를 따라잡으려 하지 않고 위상을
        // 현재로 되맞춘다. 따라잡기를 하면 늦은 뒤에 tick이 몰아쳐 더 나빠진다.
        let now = clock.now();
        if deadline < now {
            deadline = now;
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::{GroupWorker, VecSink};
    use crate::config::GroupPlan;
    use crate::error::ConfigError;
    use crate::event::{Category, HazardEvent, HazardTypeId, ModuleId, Severity};
    use crate::signal::{Signal, SignalBus, SignalId};
    use crate::supervisor::HealthRegistry;
    use crate::time::{Duration, Instant};
    use crate::traits::{Detector, EventSink, Health, ModuleCtx, Source};

    struct ConstSource(f64);
    impl Source for ConstSource {
        fn configure(&mut self, _ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
            Ok(())
        }
        fn declares(&self) -> Vec<String> {
            vec!["x".to_owned()]
        }
        fn poll(&mut self, now: Instant, bus: &mut SignalBus) {
            bus.write(SignalId(0), Signal::Scalar(self.0), now);
        }
    }

    struct OverSource;
    impl Source for OverSource {
        fn configure(&mut self, _ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
            Ok(())
        }
        fn declares(&self) -> Vec<String> {
            vec!["x".to_owned()]
        }
        fn poll(&mut self, _now: Instant, _bus: &mut SignalBus) {
            panic!("deliberate panic");
        }
    }

    struct Threshold(f64);
    impl Detector for Threshold {
        fn configure(&mut self, _ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
            Ok(())
        }
        fn requires(&self) -> Vec<String> {
            vec!["x".to_owned()]
        }
        fn tick(&mut self, now: Instant, bus: &SignalBus, out: &mut dyn EventSink) {
            if let Some(v) = bus.read_f64(SignalId(0))
                && v > self.0
            {
                out.emit(HazardEvent::new(
                    ModuleId(1),
                    HazardTypeId(7),
                    Category::Internal,
                    Severity::Warning,
                    now,
                    Duration::from_millis(100),
                ));
            }
        }
        fn health(&self) -> Health {
            Health::Ok
        }
    }

    struct Boom;
    impl Detector for Boom {
        fn configure(&mut self, _ctx: &mut ModuleCtx<'_>) -> Result<(), ConfigError> {
            Ok(())
        }
        fn requires(&self) -> Vec<String> {
            Vec::new()
        }
        fn tick(&mut self, _now: Instant, _bus: &SignalBus, _out: &mut dyn EventSink) {
            panic!("deliberate panic");
        }
    }

    fn plan(
        sources: Vec<(ModuleId, Box<dyn Source>)>,
        dets: Vec<(ModuleId, Box<dyn Detector>)>,
    ) -> GroupPlan {
        GroupPlan {
            name: "fast".to_owned(),
            period: Duration::from_millis(1),
            queue_capacity: 16,
            sources,
            detectors: dets,
        }
    }

    #[test]
    fn source_then_detector_within_one_tick() {
        let health = HealthRegistry::new(4, 1);
        let mut w = GroupWorker::new(
            plan(
                vec![(ModuleId(0), Box::new(ConstSource(9.0)))],
                vec![(ModuleId(1), Box::new(Threshold(5.0)))],
            ),
            1,
            0,
        );
        let mut out = VecSink::new();
        w.tick(Instant::from_nanos(1_000), &health, &mut out);

        // 같은 tick 안에서 쓴 값을 같은 tick의 Detector가 본다.
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].severity, Severity::Warning);
        assert_eq!(health.module_health(ModuleId(1)), Health::Ok);
    }

    #[test]
    fn no_event_below_threshold() {
        let health = HealthRegistry::new(4, 1);
        let mut w = GroupWorker::new(
            plan(
                vec![(ModuleId(0), Box::new(ConstSource(1.0)))],
                vec![(ModuleId(1), Box::new(Threshold(5.0)))],
            ),
            1,
            0,
        );
        let mut out = VecSink::new();
        w.tick(Instant::from_nanos(1), &health, &mut out);
        assert!(out.events.is_empty());
    }

    #[test]
    fn panicking_detector_is_isolated_and_skipped_afterwards() {
        let health = HealthRegistry::new(4, 1);
        let mut w = GroupWorker::new(
            plan(
                vec![(ModuleId(0), Box::new(ConstSource(9.0)))],
                vec![
                    (ModuleId(1), Box::new(Boom)),
                    (ModuleId(2), Box::new(Threshold(5.0))),
                ],
            ),
            1,
            0,
        );
        let mut out = VecSink::new();

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        w.tick(Instant::from_nanos(1), &health, &mut out);
        w.tick(Instant::from_nanos(2), &health, &mut out);
        std::panic::set_hook(prev);

        // 패닉한 모듈만 고장으로 표시되고, 뒤의 모듈은 계속 돈다.
        assert_eq!(health.module_health(ModuleId(1)), Health::Faulted);
        assert_eq!(health.module_faults(ModuleId(1)), 1); // 두 번째 tick에서는 건너뛴다
        assert_eq!(health.module_health(ModuleId(2)), Health::Ok);
        assert_eq!(out.events.len(), 2);
    }

    #[test]
    fn panicking_source_does_not_stop_the_group() {
        let health = HealthRegistry::new(4, 1);
        let mut w = GroupWorker::new(
            plan(
                vec![
                    (ModuleId(0), Box::new(OverSource)),
                    (ModuleId(1), Box::new(ConstSource(9.0))),
                ],
                vec![(ModuleId(2), Box::new(Threshold(5.0)))],
            ),
            1,
            0,
        );
        let mut out = VecSink::new();

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        w.tick(Instant::from_nanos(1), &health, &mut out);
        std::panic::set_hook(prev);

        assert_eq!(health.module_health(ModuleId(0)), Health::Faulted);
        assert_eq!(out.events.len(), 1);
    }
}
