//! Agent trait — 所有成员（LLM/User）的统一调用接口
//!
//! board 层面所有成员同等对待：claim → process → return。
//! 唯一区别在 process 的实现上（调 LLM vs 等 HTTP 输入）。

use anyhow::Result;
use types::{Identity, RequestTool};
use crate::member::ChatOutcome;

/// 所有成员（LLM 或 Human）都实现这个 trait
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    fn id(&self) -> &str;
    fn identity(&self) -> &Identity;

    /// 处理一个任务：prompt + 工具 → 结果 + 总结
    async fn process(
        &self,
        prompt: &str,
        task_description: &str,
        tools: &[RequestTool],
    ) -> Result<ChatOutcome>;
}
