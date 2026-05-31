//! loom / 标准库原子类型切换。
//!
//! `--cfg loom` 构建时改用 loom 的模型检查原子（用于穷尽并发交错验证），否则用 `core` 原子。
//! `Ordering` 两者同源，直接用 `core::sync::atomic::Ordering` 即可。

#[cfg(not(loom))]
pub(crate) use core::sync::atomic::{AtomicBool, AtomicPtr};
#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, AtomicPtr};
