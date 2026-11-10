use crate::MclError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MclInstruction {
    pub text: String,
    pub arguments: Vec<Value>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum MclOperation {
    Import {
        kind: ImportKind,
        locator: String,
        alias: String,
    },
    Create {
        name: String,
        fields: Vec<String>,
        temporary: bool,
    },
    Get(MessageExpression),
    Inject {
        source: MessageParameter,
        target: Selector,
    },
    Config(ConfigOperation),
    Effect {
        name: String,
        arguments: Vec<EffectParameter>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportKind {
    Message,
    Tool,
    Sandbox,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigOperation {
    Get,
    Set { path: String, value: toml::Value },
    Delete { path: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub block: String,
    pub field: String,
    pub range: Selection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    All,
    Index(i64),
    Range(i64, i64),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageExpression {
    pub sources: Vec<MessageSource>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageSource {
    PromptAlias(String),
    Selector(Selector),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageParameter {
    Expression(MessageExpression),
    Manual(usize),
    Empty,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectParameter {
    Expression(MessageExpression),
    Manual(usize),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MclValue {
    Unit,
    Array(MessageArray),
    Config(toml::Table),
    BlockSnapshots(Value),
    Inference {
        message: Value,
        stop_reason: String,
    },
    Import {
        kind: ImportKind,
        alias: String,
    },
    /// A recoverable MCL failure represented as an AgentMessage of type `error`.
    Error(Value),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessageArray {
    Message(Vec<Value>),
}

#[derive(Clone, Default)]
pub struct MclDrain {
    inner: Arc<DrainInner>,
}
#[derive(Default)]
struct DrainInner {
    state: Mutex<DrainState>,
    changed: Notify,
}
#[derive(Default)]
struct DrainState {
    outstanding: usize,
    error: Option<MclError>,
}
impl MclDrain {
    pub fn started(&self) {
        self.inner.state.lock().unwrap().outstanding += 1;
    }
    pub fn completed(&self, error: Option<MclError>) {
        let mut state = self.inner.state.lock().unwrap();
        state.outstanding = state.outstanding.saturating_sub(1);
        if error.is_some() {
            state.error = error;
        }
        drop(state);
        self.inner.changed.notify_waiters();
    }
    pub async fn wait(&self) -> Result<(), MclError> {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.inner.state.lock().unwrap();
                if let Some(error) = &state.error {
                    return Err(error.clone());
                }
                if state.outstanding == 0 {
                    return Ok(());
                }
            }
            notified.await;
        }
    }
}
