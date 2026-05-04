use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// 从人类用户收到的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    pub content: String,
}

/// 发给人类用户的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub content: String,
}

/// 人类输入通道抽象
#[async_trait]
pub trait HumanChannel: Send + Sync {
    async fn send(&self, msg: OutboundMessage) -> Result<()>;
    async fn recv(&mut self) -> Result<InboundMessage>;
}

/// 标准输入输出实现
pub struct StdinChannel {
    reader: BufReader<tokio::io::Stdin>,
}

impl StdinChannel {
    pub fn new() -> Self {
        Self {
            reader: BufReader::new(tokio::io::stdin()),
        }
    }
}

impl Default for StdinChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HumanChannel for StdinChannel {
    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        let mut stdout = tokio::io::stdout();
        stdout.write_all(msg.content.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<InboundMessage> {
        let mut line = String::new();
        self.reader.read_line(&mut line).await?;

        if line.is_empty() {
            anyhow::bail!("stdin closed");
        }

        Ok(InboundMessage {
            content: line.trim_end().to_string(),
        })
    }
}
