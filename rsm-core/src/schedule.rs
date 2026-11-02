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
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::config::{BuiltPipeline, GroupPlan, RecommendedActionConfig};
use crate::error::ConfigError;
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

    /// 설정한 `SCHED_FIFO` 우선순위.
    pub fn rt_priority(&self) -> Option<u8> {
        self.plan.rt_priority
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
    actions: Option<RecommendedActionConfig>,
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
            actions: None,
        }
    }

    /// 권고 액션 매핑을 건다 (R-18). `None`이면 권고는 늘 `none`.
    #[must_use]
    pub fn with_recommended_action(mut self, actions: Option<RecommendedActionConfig>) -> Self {
        self.actions = actions;
        self
    }

    /// 한 tick. 큐를 모두 비우고, 평가하고, 필요하면 출력한다.
    pub fn tick(&mut self, now: Instant, availability: MonitorAvailability) {
        for c in &mut self.consumers {
            while let Ok(ev) = c.pop() {
                self.policy.ingest(ev);
            }
        }

        let mut state = self.policy.evaluate(now, availability);
        // 권고는 (수준, 가용성)에서 결정되므로 아래 변화 판정 키에 따로 넣지 않는다.
        if let Some(a) = &self.actions {
            state.recommended_action = a.resolve(state.level, state.availability);
        }
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

/// 다음 tick 을 시작할 예정 시각(release)을 정한다.
///
/// 결과는 항상 `release + k·period` (k ≥ 1) 격자 위에 있다. 즉 위상이 유지된다.
///
/// - 조금 늦었으면(한 주기 안) 다음 칸을 그대로 쓴다. 이미 지났으면 곧바로 시작한다.
/// - 한 주기 이상 통째로 놓쳤으면 놓친 칸은 **건너뛰고**, 지금 시각 바로 앞의 칸을 쓴다.
///   밀린 tick 을 몰아서 돌리면(따라잡기) 늦은 뒤에 부하가 몰려 더 나빠진다.
///
/// 어느 경우든 시작 지연은 한 주기 미만이다.
///
/// 예전 코드는 "예정 시각을 지났으면 예정 시각 = 지금"으로 위상을 되맞췄다.
/// `thread::sleep`은 항상 조금 늦게 깨므로 이 조건이 매 tick 발동했고, 그만큼
/// 주기가 길어졌다(1 kHz 설정이 Linux 917 Hz, Windows 533 Hz).
fn next_release(release: Instant, now: Instant, period: Duration) -> Instant {
    let next = release + period;
    if now < next + period {
        return next;
    }
    let skipped = (now - next)
        .as_nanos()
        .checked_div(period.as_nanos())
        .unwrap_or(0);
    if skipped == 0 {
        return now; // 주기 0 — 격자가 없으니 지금 바로
    }
    next + period * skipped
}

/// 절대 시각까지 잔다. 이미 지났으면 바로 돌아온다.
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
///
/// # Errors
///
/// 스레드를 못 띄웠거나 그룹의 `rt_priority`를 적용하지 못하면, 이미 띄운 스레드를
/// 거두고 [`ConfigError::Startup`]을 돌려준다. 일부만 도는 파이프라인은 만들지 않는다.
pub fn run(
    pipeline: BuiltPipeline,
    clock: &Arc<dyn Clock>,
    policy: Box<dyn ArbiterPolicy>,
    sink: Box<dyn Sink>,
) -> Result<Runtime, ConfigError> {
    let BuiltPipeline {
        groups,
        names,
        signal_count,
        module_count,
        arbiter_period,
        supervisor_period,
        group_stale_after,
        recommended_action,
        sink: sink_cfg,
    } = pipeline;

    let health = Arc::new(HealthRegistry::new(module_count, groups.len()));
    let names = Arc::new(names);
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(groups.len() + 2);
    let mut consumers = Vec::with_capacity(groups.len());

    for (index, plan) in groups.into_iter().enumerate() {
        let (tx, rx) = RingBuffer::<HazardEvent>::new(plan.queue_capacity);
        consumers.push(rx);
        let worker = GroupWorker::new(plan, signal_count, index);
        let out = QueueSink::new(tx, index, Arc::clone(&health));
        let spawned = spawn_group(
            worker,
            index,
            Arc::clone(clock),
            Arc::clone(&health),
            out,
            Arc::clone(&stop),
        );
        keep_or_abort(&mut handles, &stop, spawned)?;
    }

    let mut arbiter = ArbiterWorker::new(
        policy,
        consumers,
        sink,
        Arc::clone(&names),
        sink_cfg.on_change_only,
    )
    .with_recommended_action(recommended_action);
    let (clock_a, health_a, stop_a) = (Arc::clone(clock), Arc::clone(&health), Arc::clone(&stop));
    let spawned = spawn_named("rsm-arbiter", move || {
        let mut release = clock_a.now();
        while !stop_a.load(Ordering::Relaxed) {
            arbiter.tick(clock_a.now(), health_a.availability());
            release = next_release(release, clock_a.now(), arbiter_period);
            sleep_until(&*clock_a, release);
        }
    });
    keep_or_abort(&mut handles, &stop, spawned)?;

    let (clock_s, health_s, stop_s) = (Arc::clone(clock), Arc::clone(&health), Arc::clone(&stop));
    let spawned = spawn_named("rsm-supervisor", move || {
        let mut release = clock_s.now();
        while !stop_s.load(Ordering::Relaxed) {
            health_s.scan(clock_s.now(), group_stale_after);
            release = next_release(release, clock_s.now(), supervisor_period);
            sleep_until(&*clock_s, release);
        }
    });
    keep_or_abort(&mut handles, &stop, spawned)?;

    Ok(Runtime {
        stop,
        handles,
        health,
    })
}

/// 그룹 스레드를 띄운다. 스레드가 `rt_priority`를 자기 자신에게 건 결과를 받은 뒤에 돌아온다.
///
/// 스케줄링 정책은 스레드가 스스로 바꾸는 것이 가장 단순하다. 실패하면 그 스레드는
/// 루프에 들어가지 않고 끝나며, 여기서 거둔다.
fn spawn_group(
    worker: GroupWorker,
    index: usize,
    clock: Arc<dyn Clock>,
    health: Arc<HealthRegistry>,
    out: QueueSink,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, ConfigError> {
    let name = format!("rsm-group-{}", worker.name());
    let rt_priority = worker.rt_priority();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

    let h = spawn_named(&name, move || {
        let applied = rt_priority.map_or(Ok(()), set_realtime_priority);
        let ok = applied.is_ok();
        drop(ready_tx.send(applied));
        if ok {
            group_loop(worker, index, &*clock, &health, out, &stop);
        }
    })?;
    let ready = ready_rx
        .recv()
        .unwrap_or_else(|_| Err("the thread exited before starting".to_owned()));
    match ready {
        Ok(()) => Ok(h),
        Err(reason) => {
            drop(h.join());
            Err(ConfigError::Startup {
                thread: name,
                reason,
            })
        }
    }
}

/// 이름 붙은 스레드를 띄운다. 실패를 [`ConfigError::Startup`]으로 바꾼다.
fn spawn_named(
    name: &str,
    f: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>, ConfigError> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(f)
        .map_err(|e| ConfigError::Startup {
            thread: name.to_owned(),
            reason: e.to_string(),
        })
}

/// 띄운 스레드를 모은다. 하나라도 실패하면 이미 띄운 것을 멈추고 거둔 뒤 오류를 넘긴다.
fn keep_or_abort(
    handles: &mut Vec<JoinHandle<()>>,
    stop: &AtomicBool,
    spawned: Result<JoinHandle<()>, ConfigError>,
) -> Result<(), ConfigError> {
    match spawned {
        Ok(h) => {
            handles.push(h);
            Ok(())
        }
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            for h in handles.drain(..) {
                drop(h.join());
            }
            Err(e)
        }
    }
}

/// 이 스레드를 `SCHED_FIFO` 우선순위 `level`(1~99)로 바꾼다 (TECH_STACK T5).
///
/// 같은 우선순위의 일반 프로세스와 CPU를 나눠 쓰면 스케줄러 시간 조각(수 ms)만큼
/// 깨어남이 밀린다. 실시간 정책 스레드는 일반 스레드보다 항상 먼저 돈다. 펌웨어로
/// 치면 감시 루프를 가장 높은 인터럽트 우선순위에 두는 것이다.
#[cfg(target_os = "linux")]
fn set_realtime_priority(level: u8) -> Result<(), String> {
    use thread_priority::{
        RealtimeThreadSchedulePolicy, ThreadPriority, ThreadPriorityValue, ThreadSchedulePolicy,
        set_thread_priority_and_policy, thread_native_id,
    };

    let value = ThreadPriorityValue::try_from(level)?;
    set_thread_priority_and_policy(
        thread_native_id(),
        ThreadPriority::Crossplatform(value),
        ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo),
    )
    .map_err(|e| match e {
        // EPERM. root 가 아니면 CAP_SYS_NICE 나 rtprio 한도(ulimit -r)가 있어야 한다.
        thread_priority::Error::OS(1) => format!(
            "SCHED_FIFO {level}: permission denied (needs root, CAP_SYS_NICE, or an rtprio limit)"
        ),
        other => format!("SCHED_FIFO {level}: {other}"),
    })
}

/// Linux 가 아니면 실시간 우선순위를 지원하지 않는다. 설정했으면 기동 실패다.
#[cfg(not(target_os = "linux"))]
fn set_realtime_priority(level: u8) -> Result<(), String> {
    Err(format!(
        "rt_priority {level}: real-time priority is supported only on Linux"
    ))
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
    // 이 tick 이 시작하기로 예정된 시각. 데드라인은 `release + period`다.
    let mut release = clock.now();

    while !stop.load(Ordering::Relaxed) {
        let started = clock.now();
        worker.tick(started, health, &mut out);
        let ended = clock.now();
        health.record_group_tick(index, release, started, ended, period);

        release = next_release(release, ended, period);
        sleep_until(clock, release);
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "in tests, a failed assertion is the diagnostic"
)]
mod tests {
    use super::{GroupWorker, VecSink, next_release};
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
            rt_priority: None,
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

    // ── next_release: 시각 계산만 떼어 냈으므로 스레드 없이 시험한다 ──────────

    const P: Duration = Duration::from_micros(1_000);

    fn us(v: u64) -> Instant {
        Instant::from_nanos(v * 1_000)
    }

    #[test]
    fn on_time_tick_advances_one_period() {
        assert_eq!(next_release(us(0), us(10), P), us(1_000));
    }

    #[test]
    fn oversleeping_does_not_shift_the_phase() {
        // sleep 이 60 µs 늦게 깼다. 예전 코드는 여기서 예정 시각을 1_060 으로 옮겼고,
        // 그 오차가 매 tick 쌓여 주기가 길어졌다.
        let r1 = next_release(us(0), us(10), P);
        let r2 = next_release(r1, us(1_060 + 10), P);
        assert_eq!(r2, us(2_000));
    }

    #[test]
    fn slightly_late_tick_starts_immediately_on_the_same_grid() {
        // 다음 칸(1_000)을 이미 지났지만 한 주기 안이다 → 그 칸을 그대로 쓴다
        assert_eq!(next_release(us(0), us(1_400), P), us(1_000));
    }

    #[test]
    fn whole_missed_periods_are_skipped_not_replayed() {
        // 3.5 주기 동안 멈췄다. 놓친 칸(1_000, 2_000)은 건너뛰고 3_000 을 쓴다.
        assert_eq!(next_release(us(0), us(3_500), P), us(3_000));
    }

    #[test]
    fn release_always_stays_on_the_grid() {
        let mut release = us(0);
        for now in [10, 1_070, 2_950, 7_200, 7_999, 8_000, 12_345] {
            release = next_release(release, us(now), P);
            assert_eq!(
                release.as_nanos() % P.as_nanos(),
                0,
                "off grid at now={now}"
            );
            assert!(us(now) < release + P, "lateness must stay under one period");
        }
    }

    #[test]
    fn zero_period_does_not_divide_by_zero() {
        assert_eq!(next_release(us(5), us(9), Duration::ZERO), us(9));
    }

    // ── 권고 액션 (R-18, PRD D27) ─────────────────────────────────────────────

    use super::ArbiterWorker;
    use crate::arbiter::MaxSeverity;
    use crate::config::{LevelActions, RecommendedActionConfig};
    use crate::event::{MonitorAvailability, RecommendedAction, SafetyState};
    use crate::intern::Names;
    use crate::traits::Sink;
    use std::sync::{Arc, Mutex};

    /// Sink 가 받은 상태의 (수준, 가용성, 권고) 기록.
    type Log = Arc<Mutex<Vec<(Severity, MonitorAvailability, RecommendedAction)>>>;

    /// 받은 상태를 [`Log`]에 모은다.
    struct Seen(Log);
    impl Sink for Seen {
        fn publish(&mut self, state: &SafetyState, _names: &Names) {
            self.0.lock().unwrap().push((
                state.level,
                state.availability,
                state.recommended_action,
            ));
        }
    }

    fn critical_event(at: Instant) -> HazardEvent {
        HazardEvent::new(
            ModuleId(0),
            HazardTypeId(0),
            Category::Internal,
            Severity::Critical,
            at,
            Duration::from_millis(50),
        )
    }

    fn arbiter_with(
        actions: Option<RecommendedActionConfig>,
    ) -> (ArbiterWorker, rtrb::Producer<HazardEvent>, Log) {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let worker = ArbiterWorker::new(
            Box::new(MaxSeverity::new()),
            vec![rx],
            Box::new(Seen(Arc::clone(&seen))),
            Arc::new(Names::new()),
            false,
        )
        .with_recommended_action(actions);
        (worker, tx, seen)
    }

    const ACTIONS: RecommendedActionConfig = RecommendedActionConfig {
        by_level: LevelActions {
            advisory: RecommendedAction::None,
            warning: RecommendedAction::SlowDown,
            critical: RecommendedAction::Stop,
        },
        on_unavailable: RecommendedAction::Stop,
    };

    #[test]
    fn arbiter_fills_the_recommendation_from_the_mapping() {
        use MonitorAvailability::{Available, Unavailable};

        let (mut arb, mut tx, seen) = arbiter_with(Some(ACTIONS));
        let t0 = Instant::ZERO;
        arb.tick(t0, Available); //                   위험 없음
        tx.push(critical_event(t0)).unwrap();
        arb.tick(t0, Available); //                   CRITICAL
        arb.tick(t0 + Duration::from_millis(100), Unavailable); // TTL 지남 + 감시 불가

        let seen = seen.lock().unwrap();
        assert_eq!(
            *seen,
            vec![
                (Severity::None, Available, RecommendedAction::None),
                (Severity::Critical, Available, RecommendedAction::Stop),
                // 수준은 NONE 이지만 감시 불가라 그 NONE 을 믿을 수 없다 → on_unavailable
                (Severity::None, Unavailable, RecommendedAction::Stop),
            ]
        );
    }

    #[test]
    fn without_the_section_the_recommendation_is_always_none() {
        let (mut arb, mut tx, seen) = arbiter_with(None);
        tx.push(critical_event(Instant::ZERO)).unwrap();
        arb.tick(Instant::ZERO, MonitorAvailability::Unavailable);
        assert_eq!(
            seen.lock().unwrap()[0],
            (
                Severity::Critical,
                MonitorAvailability::Unavailable,
                RecommendedAction::None
            )
        );
    }
}
