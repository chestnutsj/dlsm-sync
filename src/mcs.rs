//! MCS 公平排队锁实现。
//!
//! MCS 锁是一种基于本地变量自旋的排队锁:
//! - 每个等待者贡献一个 [`McsNode`], 在自己的节点上自旋, 避免全局 `tail` 的 cache 颠簸
//! - 入队为 atomic swap, 出队为 atomic `compare_exchange` + 链表传递
//! - 适合作为 DLSM 中"CAS-then-MCS"两阶段的第二阶段: 当 CAS 重试次数超过阈值
//!   退化为 MCS 时, 大量等待者按 FIFO 排队, 防止活锁与饥饿
//!
//! ## 节点生命周期
//!
//! 调用 [`McsLock::lock`] 时传入的 `&mut McsNode` 必须在持锁与 unlock 期间保持有效
//! 且地址稳定。本实现通过 `McsGuard<'a>` 借用引用来强制这一点: 在 guard 存活期间
//! 节点不可被 move/drop。返回上一节点的 `next` 写入逻辑要求 `prev_node` 一直存活,
//! 这同样由其持有的 `McsGuard` 借用保证。

use core::marker::PhantomData;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

/// 锁等待循环里的"让步"策略，用于把等待行为注入到锁中。
///
/// 默认 [`SpinPark`] 退化为 `spin_loop`，适合普通 OS 线程。协程运行时可提供调用
/// `yield_now` 的实现（见 `dlsm-greenthread` 的 `CoroutinePark`），使等待者让出调度器而非空转。
/// 通过该 trait 注入，`dlsm-sync` 无需依赖 `dlsm-greenthread`。
pub trait Park {
    /// 在等待循环中被反复调用一次（应让出/暂停极短时间）。
    fn park();
}

/// 默认让步策略：`core::hint::spin_loop`。
#[derive(Debug, Clone, Copy, Default)]
pub struct SpinPark;

impl Park for SpinPark {
    #[inline]
    fn park() {
        core::hint::spin_loop();
    }
}

/// MCS 锁的等待节点。
///
/// 调用方需为每次 [`McsLock::lock`] 调用提供一个独立的节点（典型做法是栈上分配
/// 一个 `let mut node = McsNode::new();`，或在协程私有 SHM 区中持有一个）。
#[repr(C, align(64))]
#[derive(Debug)]
pub struct McsNode {
    /// 指向队列中下一个等待者的节点（`null` 表示当前是队尾）。
    next: AtomicPtr<McsNode>,
    /// 自旋标志: `true` 表示尚未轮到、需继续等待。
    locked: AtomicBool,
}

impl McsNode {
    /// 构造一个新的等待节点。
    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            locked: AtomicBool::new(false),
        }
    }
}

impl Default for McsNode {
    fn default() -> Self {
        Self::new()
    }
}

/// MCS 公平排队锁，等待策略由 `P: Park` 注入（默认 [`SpinPark`]）。
///
/// 内部仅持有一个原子的 `tail` 指针；获取锁时将新节点 swap 入 tail，
/// 若 prev 非空则在 prev 的 `next` 处链入并按 `P::park` 等待 prev 释放。
#[derive(Debug)]
pub struct McsLock<P: Park = SpinPark> {
    tail: AtomicPtr<McsNode>,
    _park: PhantomData<fn() -> P>,
}

// SAFETY: 锁只持有原子指针；`P` 仅以关联函数形式使用、不被存储（PhantomData<fn()->P>），
// 故 Send/Sync 与 P 无关。节点的并发访问通过算法约束保证（详见 lock/Drop 的注释）。
unsafe impl<P: Park> Send for McsLock<P> {}
unsafe impl<P: Park> Sync for McsLock<P> {}

impl McsLock<SpinPark> {
    /// 创建一个未被持有的 MCS 锁（默认 [`SpinPark`] 等待策略）。
    ///
    /// 自定义等待策略用 [`McsLock::with_park`]（如 `McsLock::<CoroutinePark>::with_park()`）。
    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self::with_park()
    }
}

impl<P: Park> McsLock<P> {
    /// 创建一个未被持有、等待策略为 `P` 的 MCS 锁。
    #[must_use]
    #[inline]
    pub const fn with_park() -> Self {
        Self {
            tail: AtomicPtr::new(ptr::null_mut()),
            _park: PhantomData,
        }
    }

    /// 获取锁，返回一个 RAII guard；guard 离开作用域时释放锁。
    ///
    /// `node` 在 guard 存活期间必须保持有效且不能被 move。借用检查器通过
    /// `'a` 生命周期强制这一点。等待期间反复调用 `P::park`。
    pub fn lock<'a>(&'a self, node: &'a mut McsNode) -> McsGuard<'a, P> {
        // 初始化节点状态
        node.next.store(ptr::null_mut(), Ordering::Relaxed);
        node.locked.store(true, Ordering::Relaxed);

        let node_ptr: *mut McsNode = node;
        // 把自己 swap 入 tail, 获得前一位（若有）
        let prev = self.tail.swap(node_ptr, Ordering::AcqRel);

        if prev.is_null() {
            // 队列为空: 我们直接持有锁
            node.locked.store(false, Ordering::Relaxed);
        } else {
            // SAFETY: prev 节点的拥有线程持有 McsGuard, 节点保持存活直至其 Drop;
            // 我们在自己的 locked 上自旋, prev 释放时将写我们的 locked = false
            unsafe { (*prev).next.store(node_ptr, Ordering::Release) };
            while node.locked.load(Ordering::Acquire) {
                P::park();
            }
        }

        McsGuard {
            lock: self,
            node,
            _park: PhantomData,
        }
    }
}

impl<P: Park> Default for McsLock<P> {
    fn default() -> Self {
        Self::with_park()
    }
}

/// RAII 持锁守卫，离开作用域时释放锁。
#[derive(Debug)]
pub struct McsGuard<'a, P: Park = SpinPark> {
    lock: &'a McsLock<P>,
    node: &'a mut McsNode,
    _park: PhantomData<fn() -> P>,
}

impl<P: Park> Drop for McsGuard<'_, P> {
    fn drop(&mut self) {
        let node_ptr: *mut McsNode = self.node;
        let next = self.node.next.load(Ordering::Acquire);
        if next.is_null() {
            // 看似没有后继, 尝试 CAS 把 tail 还原为 null
            if self
                .lock
                .tail
                .compare_exchange(
                    node_ptr,
                    ptr::null_mut(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return;
            }
            // 有线程已 swap 进来但尚未写完我们的 next, 等它写完
            loop {
                let n = self.node.next.load(Ordering::Acquire);
                if !n.is_null() {
                    // SAFETY: n 指向新等待者的节点, 其拥有线程正在自旋且节点存活
                    unsafe { (*n).locked.store(false, Ordering::Release) };
                    return;
                }
                P::park();
            }
        } else {
            // SAFETY: next 指向后继节点, 其拥有线程正在自旋且节点存活
            unsafe { (*next).locked.store(false, Ordering::Release) };
        }
    }
}
