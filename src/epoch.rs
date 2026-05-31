//! Epoch-based reclamation（EBR）：无锁数据结构的安全延迟回收。
//!
//! 读写者操作前 [`EpochGc::pin`] 取得 [`Guard`]，记录当前 epoch；从结构上摘除的对象经
//! [`EpochGc::retire`] 登记到当前 epoch 的待回收列表。[`EpochGc::collect`] 推进 epoch 并仅
//! 回收"所有存活 guard 都晚于其 retire epoch"的对象，从而保证不释放仍可能被引用的对象。
//!
//! ## 正确性论证
//!
//! 对象 O 于 epoch `R` 被 retire（此刻已从结构摘除）。某 guard 于 epoch `P` pin：仅当 `P <= R`
//! （pin 不晚于摘除）时它才可能持有 O 的引用。故 O 可安全回收的充要条件是**不存在 `P <= R`
//! 的存活 guard**，即 `R < min(所有存活 guard 的 P)`。`collect` 正是据此回收。
//!
//! ## 实现说明
//!
//! 本实现以单 Mutex 串行化 `pin`/`retire`/`collect`，正确性显然、便于模型检查；无锁快路径
//! （per-thread 原子 epoch）留作后续性能优化。

use std::sync::{Mutex, PoisonError};

/// 待回收对象的延迟析构闭包。
type Deferred = Box<dyn FnOnce() + Send + 'static>;

struct Inner {
    /// 当前全局 epoch（单调递增）。
    epoch: usize,
    /// 存活 guard 的 pin epoch 多重集合（drop 时移除一个）。
    pinned: Vec<usize>,
    /// 待回收对象：`(retire 时的 epoch, 延迟析构)`。
    garbage: Vec<(usize, Deferred)>,
}

/// Epoch GC 域。可经 `Arc` 跨线程共享。
pub struct EpochGc {
    inner: Mutex<Inner>,
}

impl core::fmt::Debug for EpochGc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let g = self.lock();
        f.debug_struct("EpochGc")
            .field("epoch", &g.epoch)
            .field("pinned", &g.pinned.len())
            .field("garbage", &g.garbage.len())
            .finish()
    }
}

impl EpochGc {
    /// 创建一个空的 epoch GC 域。
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                epoch: 0,
                pinned: Vec::new(),
                garbage: Vec::new(),
            }),
        }
    }

    #[inline]
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 进入临界区，返回 [`Guard`]；其存活期间，retire epoch 早于本次 pin epoch 的对象可被回收，
    /// 不早于的对象则被本 guard 保护。
    pub fn pin(&self) -> Guard<'_> {
        let mut inner = self.lock();
        let epoch = inner.epoch;
        inner.pinned.push(epoch);
        Guard { gc: self, epoch }
    }

    /// 登记一个从结构摘除的对象，延迟到安全时回收。
    pub fn retire<F>(&self, reclaim: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut inner = self.lock();
        let epoch = inner.epoch;
        inner.garbage.push((epoch, Box::new(reclaim)));
    }

    /// 推进 epoch，但不回收（用于把 retire 推到更早的代）。
    pub fn advance(&self) {
        self.lock().epoch += 1;
    }

    /// 推进 epoch 并回收所有安全对象（retire epoch < 所有存活 guard 的最小 pin epoch）。
    pub fn collect(&self) {
        let ready: Vec<Deferred> = {
            let mut inner = self.lock();
            inner.epoch += 1;
            // 无存活 guard 时阈值取当前 epoch：新 guard 只会 pin 在 >= 当前 epoch，
            // 故 retire epoch < 当前 epoch 的对象对任何未来 guard 都不可达。
            let threshold = inner.pinned.iter().copied().min().unwrap_or(inner.epoch);
            let mut keep = Vec::new();
            let mut ready = Vec::new();
            for (retire_epoch, deferred) in inner.garbage.drain(..) {
                if retire_epoch < threshold {
                    ready.push(deferred);
                } else {
                    keep.push((retire_epoch, deferred));
                }
            }
            inner.garbage = keep;
            ready
        };
        // 在锁外运行析构，避免回收逻辑重入 GC 造成死锁。
        for deferred in ready {
            deferred();
        }
    }
}

impl Default for EpochGc {
    fn default() -> Self {
        Self::new()
    }
}

/// 临界区守卫；存活期间保护 retire epoch 不早于其 pin epoch 的对象。
#[derive(Debug)]
pub struct Guard<'a> {
    gc: &'a EpochGc,
    epoch: usize,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut inner = self.gc.lock();
        if let Some(pos) = inner.pinned.iter().position(|&e| e == self.epoch) {
            inner.pinned.swap_remove(pos);
        }
    }
}
