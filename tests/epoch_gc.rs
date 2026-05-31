//! Epoch-based reclamation 测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use dlsm_sync::EpochGc;

#[test]
fn unpinned_retired_object_is_reclaimed_on_collect() {
    let gc = EpochGc::new();
    let freed = Arc::new(AtomicBool::new(false));

    let f = Arc::clone(&freed);
    gc.retire(move || f.store(true, Ordering::SeqCst));
    // 无任何存活 guard → 一次 collect 即可回收
    gc.collect();
    assert!(
        freed.load(Ordering::SeqCst),
        "无 pin 时 retire 的对象应被回收"
    );
}

#[test]
fn pinned_guard_delays_reclamation() {
    let gc = EpochGc::new();
    let freed = Arc::new(AtomicBool::new(false));

    {
        let _guard = gc.pin(); // 在 retire 之前 pin → 可能持有该对象引用
        let f = Arc::clone(&freed);
        gc.retire(move || f.store(true, Ordering::SeqCst));
        gc.collect();
        assert!(
            !freed.load(Ordering::SeqCst),
            "guard 在 retire 前 pin，存活期间不得回收"
        );
    }
    // guard 释放后再回收
    gc.collect();
    assert!(freed.load(Ordering::SeqCst), "guard 释放后应被回收");
}

#[test]
fn guard_pinned_after_retirement_does_not_delay() {
    let gc = EpochGc::new();
    let freed = Arc::new(AtomicBool::new(false));

    let f = Arc::clone(&freed);
    gc.retire(move || f.store(true, Ordering::SeqCst)); // 于 epoch 0 retire
    gc.advance(); // 推进到 epoch 1（不回收）

    let _late = gc.pin(); // 于 epoch 1 pin —— 晚于 retire，不可能引用该对象
    gc.collect();
    assert!(
        freed.load(Ordering::SeqCst),
        "晚于 retire 的 guard 不应阻止回收"
    );
}

#[test]
fn concurrent_pin_retire_collect_reclaims_everything_once() {
    const THREADS: usize = 8;
    const ITERS: usize = 500;

    let gc = Arc::new(EpochGc::new());
    let drops = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let gc = Arc::clone(&gc);
            let drops = Arc::clone(&drops);
            thread::spawn(move || {
                for i in 0..ITERS {
                    let guard = gc.pin();
                    let d = Arc::clone(&drops);
                    gc.retire(move || {
                        d.fetch_add(1, Ordering::SeqCst);
                    });
                    drop(guard);
                    if (t + i) % 10 == 0 {
                        gc.collect();
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // 末尾多收几次，确保全部排空。
    for _ in 0..8 {
        gc.collect();
    }
    assert_eq!(
        drops.load(Ordering::SeqCst),
        THREADS * ITERS,
        "每个 retire 的对象应恰好被回收一次（无泄漏、无重复）"
    );
}
