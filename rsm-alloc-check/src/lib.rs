//! 시험 전용 할당 카운터 — `fast` 그룹 tick 경로의 힙 할당 0을 기계로 확인한다 (PRD R-08).
//!
//! # 왜 별도 크레이트인가
//!
//! 힙 할당을 세려면 전역 할당자([`GlobalAlloc`])를 갈아끼워야 한다. 모든 `Box`·`Vec`·
//! `String`이 이 할당자를 거치므로 그 안에서 횟수를 올리면 된다. C로 치면 링커의
//! `--wrap=malloc`으로 `malloc`/`free`를 가로채 세는 시험용 오브젝트다.
//!
//! 문제는 [`GlobalAlloc`]이 `unsafe` 트레이트라는 점이다. 다른 크레이트는 모두
//! `unsafe_code = "forbid"`라서 여기에 격리했다. 배포하지 않는다(`publish = false`).
//!
//! # 쓰는 법
//!
//! ```
//! let (v, n) = rsm_alloc_check::count(|| 1 + 1);
//! assert_eq!(v, 2);
//! assert!(n.is_zero());
//! ```
//!
//! 세는 것은 **[`count`]를 부른 스레드**의 힙 연산뿐이다. `cargo test`는 여러 시험을
//! 동시에 돌리므로 전역 카운터로 세면 다른 시험의 할당이 섞인다.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// 한 구간에서 일어난 힙 연산 횟수.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// `alloc` / `alloc_zeroed` 횟수.
    pub allocs: u64,
    /// `realloc` 횟수.
    pub reallocs: u64,
    /// `dealloc` 횟수. 0이 아니면 어딘가에서 힙 객체가 만들어졌다 버려지고 있다.
    pub deallocs: u64,
}

impl Counts {
    const ZERO: Self = Self {
        allocs: 0,
        reallocs: 0,
        deallocs: 0,
    };

    /// 셋 다 0인가.
    pub const fn is_zero(&self) -> bool {
        self.allocs == 0 && self.reallocs == 0 && self.deallocs == 0
    }
}

thread_local! {
    // `const` 초기화이고 Drop 이 없는 타입이라, 접근해도 지연 초기화나 소멸자 등록이
    // 없다 — 즉 할당하지 않는다. 할당자 안에서 할당하면 무한 재귀가 되므로 이 성질이
    // 필수다.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<Counts> = const { Cell::new(Counts::ZERO) };
}

/// 이 스레드가 세는 중이면 횟수를 올린다.
///
/// `try_with`를 쓰는 이유: 스레드 종료 중에도 패닉하지 않는다. 할당자 안의 패닉은
/// 프로세스 abort 다.
fn bump(f: impl FnOnce(&mut Counts)) {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNTS.try_with(|c| {
            let mut v = c.get();
            f(&mut v);
            c.set(v);
        });
    }
}

/// 시스템 할당자를 감싸 횟수만 센다. 실제 할당은 [`System`]이 한다.
struct Counting;

#[allow(
    unsafe_code,
    reason = "GlobalAlloc is an unsafe trait; this impl is the only unsafe code in the workspace"
)]
// SAFETY: 모든 메서드가 받은 인자를 그대로 `System`에 넘기고 그 결과를 그대로 돌려준다.
// `System`은 GlobalAlloc 계약을 지키므로 이 래퍼도 지킨다. 덧붙인 일은 `bump` 뿐이고,
// `bump`는 할당하지 않으며(위 thread_local 주석) 패닉하지 않는다.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(|c| c.allocs += 1);
        // SAFETY: 호출자가 `alloc`의 계약(크기 0 이 아닌 layout)을 지켰고 그대로 넘긴다.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump(|c| c.allocs += 1);
        // SAFETY: `alloc`과 같다.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump(|c| c.deallocs += 1);
        // SAFETY: `ptr`은 이 할당자(곧 `System`)가 같은 `layout`으로 내준 것이라고
        // 호출자가 보장한다.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(|c| c.reallocs += 1);
        // SAFETY: `dealloc`과 같은 `ptr`·`layout` 보장에 더해, `new_size`가 계약을
        // 지킨다고 호출자가 보장한다.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// 이 크레이트를 링크한 바이너리(= 이 크레이트의 시험들)는 모두 이 할당자를 쓴다.
#[global_allocator]
static GLOBAL: Counting = Counting;

/// 구간을 벗어날 때 세기를 끈다. `f`가 패닉해도 꺼진다.
struct Window;

impl Drop for Window {
    fn drop(&mut self) {
        ACTIVE.set(false);
    }
}

/// `f`를 실행하는 동안 **이 스레드**에서 일어난 힙 연산을 센다.
///
/// 중첩해서 부르지 않는다. 안쪽 호출이 끝나면 바깥 구간의 집계가 끊긴다.
/// `f`의 반환값이 힙을 쓰면 그것도 센다. 반환값을 버리는 일은 구간 밖에서 일어난다.
pub fn count<R>(f: impl FnOnce() -> R) -> (R, Counts) {
    COUNTS.set(Counts::ZERO);
    ACTIVE.set(true);
    let window = Window;
    let r = f();
    drop(window);
    (r, COUNTS.get())
}

#[cfg(test)]
mod tests {
    //! 계측기 자체의 시험. 할당이 있을 때 **실제로 잡는지**를 먼저 확인해야
    //! "0이 나왔다"를 믿을 수 있다.

    use super::count;
    use std::hint::black_box;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_box_is_one_alloc_and_one_dealloc() {
        let (b, n) = count(|| black_box(Box::new(7_u64)));
        assert_eq!((n.allocs, n.reallocs, n.deallocs), (1, 0, 0));

        let ((), n) = count(|| drop(b));
        assert_eq!((n.allocs, n.reallocs, n.deallocs), (0, 0, 1));
    }

    #[test]
    fn vec_growth_is_a_realloc() {
        let mut v: Vec<u64> = Vec::with_capacity(1);
        v.push(1);
        let ((), n) = count(|| {
            v.push(2);
            black_box(&v);
        });
        assert_eq!(n.reallocs, 1);
    }

    #[test]
    fn plain_arithmetic_is_zero() {
        let (_, n) = count(|| black_box(3_u64).wrapping_mul(black_box(5)));
        assert!(n.is_zero());
    }

    #[test]
    fn nothing_is_counted_outside_the_window() {
        let ((), _) = count(|| ());
        let b = black_box(Box::new(1_u8)); // 구간 밖
        drop(b);
        let ((), n) = count(|| ());
        assert!(n.is_zero());
    }

    #[test]
    fn other_threads_are_not_counted() {
        // 다른 스레드가 쉬지 않고 할당하는 동안, 이 스레드는 아무것도 하지 않는다.
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    black_box(Box::new(0_u64));
                }
            })
        };
        let ((), n) = count(|| std::thread::sleep(std::time::Duration::from_millis(20)));
        stop.store(true, Ordering::Relaxed);
        let _ = worker.join();
        assert!(n.is_zero(), "counted another thread's allocations: {n:?}");
    }
}
