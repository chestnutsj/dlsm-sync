//! MCS 锁基础正确性测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::cell::UnsafeCell;
use std::sync::{Arc, Barrier};
use std::thread;

use dlsm_sync::{McsLock, McsNode};

#[test]
fn lock_then_unlock_uncontended() {
    let lock = McsLock::new();
    let mut node = McsNode::new();
    {
        let _guard = lock.lock(&mut node);
        // 临界区: 无操作, 单线程持有
    }
    // 再次获取应同样成功
    let mut node2 = McsNode::new();
    let _g2 = lock.lock(&mut node2);
}

#[test]
fn two_threads_serialize_writes() {
    struct Cell(UnsafeCell<u64>);
    // SAFETY: 仅在 McsLock 保护下访问
    unsafe impl Sync for Cell {}

    let lock = Arc::new(McsLock::new());
    let cell = Arc::new(Cell(UnsafeCell::new(0)));
    let barrier = Arc::new(Barrier::new(2));

    let handles: Vec<_> = (0..2)
        .map(|_| {
            let lock = Arc::clone(&lock);
            let cell = Arc::clone(&cell);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut node = McsNode::new();
                barrier.wait();
                for _ in 0..10_000 {
                    let _g = lock.lock(&mut node);
                    // SAFETY: lock 已持有, 唯一写者
                    unsafe {
                        *cell.0.get() += 1;
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
    // SAFETY: 所有线程已退出
    let final_value = unsafe { *cell.0.get() };
    assert_eq!(final_value, 20_000);
}

#[test]
fn high_contention_counter_is_exact() {
    struct Cell(UnsafeCell<u64>);
    unsafe impl Sync for Cell {}

    const THREADS: usize = 8;
    const OPS: u64 = 5_000;

    let lock = Arc::new(McsLock::new());
    let cell = Arc::new(Cell(UnsafeCell::new(0u64)));
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let lock = Arc::clone(&lock);
            let cell = Arc::clone(&cell);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut node = McsNode::new();
                barrier.wait();
                for _ in 0..OPS {
                    let _g = lock.lock(&mut node);
                    unsafe {
                        *cell.0.get() += 1;
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
    let final_value = unsafe { *cell.0.get() };
    assert_eq!(final_value, THREADS as u64 * OPS);
}
