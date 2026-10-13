use crate::message_types::AgentMessage;
use crate::resource_components::{CallableResource, SandboxValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InnerType {
    Message,
    Tool,
    Sandbox,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockInner {
    Message(Vec<AgentMessage>),
    Tool(Vec<CallableResource>),
    Sandbox(Vec<SandboxValue>),
}

impl BlockInner {
    pub fn empty(kind: InnerType) -> Self {
        match kind {
            InnerType::Message => Self::Message(Vec::new()),
            InnerType::Tool => Self::Tool(Vec::new()),
            InnerType::Sandbox => Self::Sandbox(Vec::new()),
        }
    }
    pub fn inner_type(&self) -> InnerType {
        match self {
            Self::Message(_) => InnerType::Message,
            Self::Tool(_) => InnerType::Tool,
            Self::Sandbox(_) => InnerType::Sandbox,
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Message(v) => v.len(),
            Self::Tool(v) => v.len(),
            Self::Sandbox(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn slice(&self, start: usize, end: usize) -> Result<Self, BlockError> {
        match self {
            Self::Message(v) => v.get(start..end).map(|v| Self::Message(v.to_vec())),
            Self::Tool(v) => v.get(start..end).map(|v| Self::Tool(v.to_vec())),
            Self::Sandbox(v) => v.get(start..end).map(|v| Self::Sandbox(v.to_vec())),
        }
        .ok_or(BlockError::InvalidRange)
    }
    pub fn append(&mut self, other: Self) -> Result<(), BlockError> {
        match (self, other) {
            (Self::Message(a), Self::Message(b)) => a.extend(b),
            (Self::Tool(a), Self::Tool(b)) => a.extend(b),
            (Self::Sandbox(a), Self::Sandbox(b)) => a.extend(b),
            _ => return Err(BlockError::TypeMismatch),
        }
        Ok(())
    }
    pub fn replace(
        &mut self,
        start: usize,
        end: usize,
        replacement: Self,
    ) -> Result<(), BlockError> {
        if start > end || end > self.len() {
            return Err(BlockError::InvalidRange);
        }
        match (self, replacement) {
            (Self::Message(a), Self::Message(b)) => {
                a.splice(start..end, b);
            }
            (Self::Tool(a), Self::Tool(b)) => {
                a.splice(start..end, b);
            }
            (Self::Sandbox(a), Self::Sandbox(b)) => {
                a.splice(start..end, b);
            }
            _ => return Err(BlockError::TypeMismatch),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockPath {
    pub block_id: String,
    pub inner_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub inners: BTreeMap<String, BlockInner>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBlock {
    blocks: BTreeMap<String, Block>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockError {
    BlockMissing,
    InnerMissing,
    DuplicateBlock,
    DuplicateField,
    TypeMismatch,
    InvalidRange,
}
impl std::fmt::Display for BlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BlockError {}
impl AgentBlock {
    pub fn create(
        &mut self,
        id: String,
        fields: Vec<(String, InnerType)>,
    ) -> Result<(), BlockError> {
        if self.blocks.contains_key(&id) {
            return Err(BlockError::DuplicateBlock);
        }
        let mut block = Block::default();
        for (name, kind) in fields {
            if block.inners.insert(name, BlockInner::empty(kind)).is_some() {
                return Err(BlockError::DuplicateField);
            }
        }
        self.blocks.insert(id, block);
        Ok(())
    }
    pub fn get(&self, path: &BlockPath) -> Result<&BlockInner, BlockError> {
        self.blocks
            .get(&path.block_id)
            .ok_or(BlockError::BlockMissing)?
            .inners
            .get(&path.inner_id)
            .ok_or(BlockError::InnerMissing)
    }
    pub fn replace(
        &mut self,
        path: &BlockPath,
        start: usize,
        end: usize,
        replacement: BlockInner,
    ) -> Result<(), BlockError> {
        self.blocks
            .get_mut(&path.block_id)
            .ok_or(BlockError::BlockMissing)?
            .inners
            .get_mut(&path.inner_id)
            .ok_or(BlockError::InnerMissing)?
            .replace(start, end, replacement)
    }
}

#[derive(Clone, Debug)]
pub enum BlockOperation {
    Create {
        id: String,
        fields: Vec<(String, InnerType)>,
    },
    Read {
        path: BlockPath,
        start: usize,
        end: Option<usize>,
    },
    Replace {
        path: BlockPath,
        start: usize,
        end: usize,
        value: BlockInner,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockOperationResult {
    Unit,
    Array(BlockInner),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_empty_arrays_round_trip_and_reject_mixed_append() {
        for kind in [InnerType::Message, InnerType::Tool, InnerType::Sandbox] {
            let value = BlockInner::empty(kind);
            assert_eq!(value.inner_type(), kind);
            assert!(value.is_empty());
            assert_eq!(value.slice(0, 0).unwrap(), value);
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<BlockInner>(&json).unwrap(), value);
        }
        let mut tools = BlockInner::empty(InnerType::Tool);
        assert_eq!(
            tools.append(BlockInner::empty(InnerType::Sandbox)),
            Err(BlockError::TypeMismatch)
        );
        assert_eq!(tools.inner_type(), InnerType::Tool);
    }
    #[test]
    fn failed_create_and_replace_are_non_mutating() {
        let mut blocks = AgentBlock::default();
        assert_eq!(
            blocks.create(
                "a".into(),
                vec![
                    ("x".into(), InnerType::Message),
                    ("x".into(), InnerType::Sandbox)
                ]
            ),
            Err(BlockError::DuplicateField)
        );
        blocks
            .create("a".into(), vec![("x".into(), InnerType::Message)])
            .unwrap();
        let path = BlockPath {
            block_id: "a".into(),
            inner_id: "x".into(),
        };
        let before = blocks.clone();
        assert_eq!(
            blocks.replace(&path, 0, 0, BlockInner::Sandbox(vec![])),
            Err(BlockError::TypeMismatch)
        );
        assert_eq!(blocks, before);
    }
}
