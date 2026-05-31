//! MCS 锁的 loom 模型检查：穷尽并发交错验证互斥。
//!
//! 仅在 `--cfg loom` 下编译运行：
//! `RUSTFLAGS="--cfg loom" cargo test -p dlsm-sync --test loom_mcs --release`

#![cfg(loom)]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use dlsm_sync::{McsLock, McsNode, Park};
use loom::cell::UnsafeCell;
use loom::sync::Arc;
use loom::thread;

/// loom 感知的等待策略：自旋时让出给 loom 调度器，避免"spin loop 分支爆炸"。
struct LoomPark;
impl Park for LoomPark {
    fn park() {
        loom::thread::yield_now();
    }
}

/// 两个线程各取锁一次并访问共享单元；loom 穷尽交错下：
/// - 互斥成立 → `UnsafeCell` 不会被并发访问（否则 loom 报 concurrent access）
/// - 最终计数恒为 2。
#[test]
fn loom_mcs_two_threads_mutual_exclusion() {
    loom::model(|| {
        let lock = Arc::new(McsLock::<LoomPark>::with_park());
        let cell = Arc::new(UnsafeCell::new(0u64));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let lock = Arc::clone(&lock);
                let cell = Arc::clone(&cell);
                thread::spawn(move || {
                    let mut node = McsNode::new();
                    let _g = lock.lock(&mut node);
                    // 临界区：经 loom UnsafeCell 访问，互斥被破坏时 loom 立即检出。
                    cell.with_mut(|p| unsafe { *p += 1 });
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let total = cell.with(|p| unsafe { *p });
        assert_eq!(total, 2);
    });
}
