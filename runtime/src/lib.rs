//! Margatroid Runtime — Agent 运行时
//!
//! Workspace 是唯一的入口点。持有成员、委托板、沙箱、SQLite 记忆，
//! 并管理所有成员的控制循环。

pub mod board;
pub mod client;
pub mod member;
pub mod memory;
pub mod workspace;

pub use board::{DelegationBoard, DelegationTask, TaskChain, TaskResult};
pub use client::Client;
pub use member::{Agent, Member};
pub use memory::SqliteMemory;
pub use workspace::{AgentEntry, Workspace, base_tools, manager_tools};
