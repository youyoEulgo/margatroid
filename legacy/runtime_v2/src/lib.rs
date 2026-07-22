//! Runtime V2 — 重构版本
//!
//! 职责分离：
//! - Kernel: 唯一入口，持有 EventBus + 配置 + workspace 列表
//! - EventBus: 全局事件通道注册表
//! - Workspace: 单个 workspace 的运行时状态
//! - DelegationBoard: 纯调度层，只管委托链 + 发布区

pub mod events;
pub mod kernel;
pub mod board;
pub mod memory;
pub mod context;
pub mod member;
pub mod workspace;
pub mod tools;
pub mod engine;

// 暂时 re-export
pub use events::EventBus;
pub use kernel::Kernel;
pub use board::{DelegationBoard, DelegationTask, TaskResult, TaskChain, BoardStatus};
pub use memory::SqliteMemory;
pub use context::{assemble_prompt, format_chain, format_worklog};
pub use member::{Agent, ChatOutcome, Member};
pub use workspace::{AgentEntry, Workspace};
pub use tools::{execute_tool, ToolExecResult, base_tools, manager_tools};
