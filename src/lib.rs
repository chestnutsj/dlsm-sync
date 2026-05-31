//! 基于共享内存的同步原语。
//!
//! 提供 MCS 公平排队锁、ticket lock、epoch-based GC 等无锁/低锁原语。
//! 所有锁节点从 `dlsm-shm` 的 Arena 分配，保证跨线程/跨进程可见。
//! 通过 trait 注入 `yield_now`，可在等待时让出协程（与 `dlsm-greenthread` 协作）
//! 或退化为 `spin_loop`（与普通 OS 线程兼容）。
//!
//! 详细设计见 `docs/superpowers/specs/2026-05-22-bwtree-design.md`。

#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod epoch;
mod mcs;

pub use epoch::{EpochGc, Guard};
pub use mcs::{McsGuard, McsLock, McsNode, Park, SpinPark};
