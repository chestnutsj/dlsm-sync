//! MCS 锁 yield 注入测试：等待循环里反复调用注入的 `Park::park`。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use dlsm_sync::{McsLock, McsNode, Park};

static PARKS: AtomicUsize = AtomicUsize::new(0);

/// 记录被调用次数的 Park，用于断言等待循环确实命中注入点。
struct CountingPark;
impl Park for CountingPark {
    fn park() {
        PARKS.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

#[test]
fn park_hook_fires_while_waiting_under_contention() {
    PARKS.store(0, Ordering::Relaxed);
    let lock = Arc::new(McsLock::<CountingPark>::with_park());
    let start = Arc::new(Barrier::new(2));

    // 主线程先持锁。
    let mut node1 = McsNode::new();
    let guard = lock.lock(&mut node1);

    let t = {
        let lock = Arc::clone(&lock);
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            let mut node2 = McsNode::new();
            // 主线程持锁中 → 本次 lock 必然进入等待循环、反复 park。
            let _g = lock.lock(&mut node2);
        })
    };

    start.wait();
    // 等到等待线程确实开始 park（命中注入点）。
    while PARKS.load(Ordering::Relaxed) == 0 {
        thread::yield_now();
    }
    // 释放锁，等待线程随后取得。
    drop(guard);
    t.join().unwrap();

    assert!(
        PARKS.load(Ordering::Relaxed) > 0,
        "等待循环应调用注入的 Park::park"
    );
}

#[test]
fn default_lock_still_works_without_explicit_park() {
    // 默认 SpinPark：不指定 P 时既有用法不变。
    let lock = McsLock::new();
    let mut node = McsNode::new();
    let _g = lock.lock(&mut node);
}
