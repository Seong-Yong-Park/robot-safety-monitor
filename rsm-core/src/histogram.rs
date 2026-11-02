//! tick 경로용 고정 크기 지연 히스토그램 — 지터 P50/P99 를 할당 없이 모은다.
//!
//! 평균과 최대만으로는 PRD §7의 "지터 P99 가 주기의 10% 이내"를 판정할 수 없다.
//! 그렇다고 tick 마다 값을 `Vec`에 쌓으면 할당이 생긴다(R-08). 그래서 칸 수가
//! 정해진 원자 카운터 배열에 **어느 칸에 들어갔는지만** 센다. 펌웨어에서 ISR
//! 지연을 재려고 고정 크기 `uint32_t hist[N]`을 두는 것과 같다.
//!
//! # 칸 배치 (로그-선형)
//!
//! ```text
//!   칸 0          : 0 ≤ v < 1024 ns (≈ 1 µs)
//!   2^e ≤ v < 2^(e+1) : 16 칸으로 등분       (e = 10 … 29)
//!   마지막 칸     : v ≥ 2^30 ns (≈ 1.07 s)
//! ```
//!
//! 칸 너비가 칸 하한의 1/16 이하이므로 **상대 오차 ≤ 6.25%** 다. 1 µs 근처든 1 ms
//! 근처든 오차 비율이 같다. 칸은 1 + 20 × 16 + 1 = 322 개, 그룹당 약 2.6 KB.
//!
//! 백분위는 그 순위가 들어간 칸의 **상한**으로 돌려준다. 실제 값보다 작게 나오는
//! 일이 없다(보수적).

use std::sync::atomic::{AtomicU64, Ordering};

/// 2배 구간 하나를 나누는 칸 수의 지수. 16 칸.
const SUB_BITS: u32 = 4;
const SUBS: usize = 1 << SUB_BITS;
/// 이 지수부터 로그 칸이다. 그 아래(1024 ns 미만)는 칸 0 하나.
const MIN_EXP: u32 = 10;
/// 이 지수 이상(2^30 ns 이상)은 마지막 칸 하나.
const MAX_EXP: u32 = 30;
const OCTAVES: usize = (MAX_EXP - MIN_EXP) as usize;

/// 전체 칸 수.
pub(crate) const BUCKETS: usize = 1 + OCTAVES * SUBS + 1;
const LAST: usize = BUCKETS - 1;

/// 값 `v`(ns)가 들어갈 칸.
fn bucket_of(v: u64) -> usize {
    if v < 1 << MIN_EXP {
        return 0;
    }
    let e = u64::BITS - 1 - v.leading_zeros(); // floor(log2 v), MIN_EXP 이상
    if e >= MAX_EXP {
        return LAST;
    }
    let sub = (v >> (e - SUB_BITS)) & (SUBS as u64 - 1);
    let octave = (e - MIN_EXP) as usize;
    1 + octave * SUBS + usize::try_from(sub).unwrap_or(SUBS - 1)
}

/// 칸 `i`에 들어갈 수 있는 가장 큰 값(ns).
fn upper_bound(i: usize) -> u64 {
    if i == 0 {
        return (1 << MIN_EXP) - 1;
    }
    if i >= LAST {
        return u64::MAX;
    }
    let j = i - 1;
    let e = MIN_EXP + u32::try_from(j / SUBS).unwrap_or(0);
    let sub = (j % SUBS) as u64;
    ((SUBS as u64 + sub + 1) << (e - SUB_BITS)) - 1
}

/// 지연 히스토그램. 쓰는 쪽(그룹 스레드)과 읽는 쪽이 달라도 락이 없다.
#[derive(Debug)]
pub(crate) struct LatencyHistogram {
    buckets: [AtomicU64; BUCKETS],
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl LatencyHistogram {
    /// 값 하나를 센다. 원자 덧셈 한 번 — 할당도 락도 없다.
    pub(crate) fn record(&self, v: u64) {
        if let Some(b) = self.buckets.get(bucket_of(v)) {
            b.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 백분위 여러 개를 한 번에 구한다. `ppm`은 백만분율 — P99 는 `990_000`.
    ///
    /// 각 결과는 그 순위가 들어간 칸의 상한이다. 비어 있으면 0.
    /// 쓰는 중에 읽으면 칸들이 조금씩 다른 시점의 값일 수 있다. 통계라 문제 삼지 않는다.
    pub(crate) fn quantiles<const K: usize>(&self, ppm: [u64; K]) -> [u64; K] {
        let mut counts = [0_u64; BUCKETS]; // 스택 위 2.6 KB — 할당 없음
        for (c, b) in counts.iter_mut().zip(&self.buckets) {
            *c = b.load(Ordering::Relaxed);
        }
        let total: u64 = counts.iter().sum();
        ppm.map(|q| {
            if total == 0 {
                return 0;
            }
            // 순위 = ceil(total × q / 10^6), 최소 1
            let rank = (u128::from(total) * u128::from(q))
                .div_ceil(1_000_000)
                .max(1);
            let mut seen = 0_u128;
            for (i, &c) in counts.iter().enumerate() {
                seen += u128::from(c);
                if seen >= rank {
                    return upper_bound(i);
                }
            }
            upper_bound(LAST)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BUCKETS, LAST, LatencyHistogram, bucket_of, upper_bound};

    /// 경계값과 그 사이를 고루 섞은 표본. 0 부터 2^31 근처까지.
    fn samples() -> Vec<u64> {
        let mut v = vec![0, 1, 1_022, 1_023, 1_024, 1_025];
        for e in 10..32 {
            let base = 1_u64 << e;
            for d in [0, 1, base / 3, base / 2, base - 1] {
                v.push(base + d);
                v.push(base - 1 + d);
            }
        }
        v.sort_unstable();
        v
    }

    #[test]
    fn layout_is_322_buckets() {
        assert_eq!(BUCKETS, 322);
    }

    #[test]
    fn edges() {
        assert_eq!(bucket_of(0), 0);
        assert_eq!(bucket_of(1_023), 0);
        assert_eq!(bucket_of(1_024), 1);
        assert_eq!(bucket_of((1 << 30) - 1), LAST - 1);
        assert_eq!(bucket_of(1 << 30), LAST);
        assert_eq!(bucket_of(u64::MAX), LAST);
    }

    #[test]
    fn buckets_never_go_backwards() {
        let s = samples();
        for w in s.windows(2) {
            assert!(bucket_of(w[0]) <= bucket_of(w[1]), "{} vs {}", w[0], w[1]);
        }
    }

    #[test]
    fn every_value_lies_inside_its_bucket() {
        for v in samples() {
            let b = bucket_of(v);
            assert!(v <= upper_bound(b), "{v} above bucket {b}");
            if b > 0 {
                assert!(v > upper_bound(b - 1), "{v} also fits bucket {}", b - 1);
            }
        }
    }

    #[test]
    fn relative_error_is_at_most_one_sixteenth() {
        for v in samples() {
            let b = bucket_of(v);
            if b == 0 || b == LAST {
                continue;
            }
            // 보고값(칸 상한) − 실제값 ≤ 실제값 / 16
            assert!(upper_bound(b) - v <= v / 16, "{v} → {}", upper_bound(b));
        }
    }

    #[test]
    fn empty_histogram_reports_zero() {
        let h = LatencyHistogram::default();
        assert_eq!(h.quantiles([500_000, 990_000]), [0, 0]);
    }

    #[test]
    fn uniform_1_to_1000_us() {
        let h = LatencyHistogram::default();
        for us in 1..=1_000 {
            h.record(us * 1_000);
        }
        let [p50, p99, p100] = h.quantiles([500_000, 990_000, 1_000_000]);
        // 실제 P50 = 500 µs, P99 = 990 µs, 최대 = 1000 µs. 보고값은 그 이상, 6.25% 이내.
        for (got, want) in [(p50, 500_000), (p99, 990_000), (p100, 1_000_000)] {
            assert!(got >= want && got - want <= want / 16, "{got} vs {want}");
        }
    }

    #[test]
    fn rare_outliers_move_the_tail_not_the_median() {
        let h = LatencyHistogram::default();
        for _ in 0..990 {
            h.record(10_000); // 10 µs
        }
        for _ in 0..10 {
            h.record(5_000_000); // 5 ms — 1%
        }
        let [p50, p99, p999] = h.quantiles([500_000, 990_000, 999_000]);
        assert!(p50 < 11_000);
        assert!(p99 < 11_000, "990번째까지는 10 µs 다");
        assert!(p999 >= 5_000_000);
    }
}
