//! 验证 MCS 锁可以构造在 SHM Arena 中, 等待节点也可以从 Arena 分配。
//!
//! 这是 dlsm-shm + dlsm-sync 的集成冒烟测试: 当 Bw-Tree 上层在 SHM 中放置
//! 行锁/页锁结构时, 整套机制需端到端工作。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use core::alloc::Layout;
use std::sync::{Arc, Barrier};
use std::thread;

use dlsm_shm::Arena;
use dlsm_sync::{McsLock, McsNode};

#[test]
fn lock_resides_in_shm_and_serializes_threads() {
    const THREADS: usize = 4;
    const OPS: u64 = 2_000;

    let arena = Arc::new(Arena::new_anonymous(4096).unwrap());

    // 在 Arena 中放置一个 MCS 锁
    let lock_ptr = {
        let layout = Layout::new::<McsLock>();
        let p = arena.alloc(layout).unwrap().cast::<McsLock>();
        // SAFETY: p 指向 Arena 内 sizeof(McsLock) 字节, 我们独占初始化
        unsafe {
            p.as_ptr().write(McsLock::new());
        }
        p
    };
    // SAFETY: 锁的存活期 == Arena 存活期; 我们通过 Arc<Arena> 让所有线程持有引用
    let lock_ref: &'static McsLock =
        unsafe { core::mem::transmute::<&McsLock, &'static McsLock>(lock_ptr.as_ref()) };

    // 共享计数器, 也放在 Arena 中
    let counter_ptr = {
        let layout = Layout::new::<core::sync::atomic::AtomicU64>();
        let p = arena
            .alloc(layout)
            .unwrap()
            .cast::<core::sync::atomic::AtomicU64>();
        unsafe {
            p.as_ptr().write(core::sync::atomic::AtomicU64::new(0));
        }
        p
    };
    let counter_ref: &'static core::sync::atomic::AtomicU64 = unsafe {
        core::mem::transmute::<&core::sync::atomic::AtomicU64, &'static _>(counter_ptr.as_ref())
    };

    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let arena = Arc::clone(&arena);
            thread::spawn(move || {
                // 每个线程的等待节点也从 Arena 分配 (模拟 SHM 跨进程场景)
                let node_layout = Layout::new::<McsNode>();
                let node_ptr = arena.alloc(node_layout).unwrap().cast::<McsNode>();
                // SAFETY: node_ptr 独占指向 sizeof(McsNode) 字节
                unsafe {
                    node_ptr.as_ptr().write(McsNode::new());
                }
                let node = unsafe { &mut *node_ptr.as_ptr() };

                barrier.wait();
                for _ in 0..OPS {
                    let _g = lock_ref.lock(node);
                    let cur = counter_ref.load(core::sync::atomic::Ordering::Relaxed);
                    counter_ref.store(cur + 1, core::sync::atomic::Ordering::Relaxed);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(
        counter_ref.load(core::sync::atomic::Ordering::Acquire),
        THREADS as u64 * OPS
    );
}
