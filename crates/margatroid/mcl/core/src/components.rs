use crate::{MclCommand, MclDrain};
use core_plugin::Component;
use std::collections::VecDeque;
use tokio_util::sync::CancellationToken;

pub struct MclQueue {
    pub commands: VecDeque<MclCommand>,
    pub pending: Option<String>,
    pub accepting: bool,
    pub allowed_imports: Vec<String>,
    pub drain: MclDrain,
    pub cancel: CancellationToken,
}
impl Default for MclQueue {
    fn default() -> Self {
        Self {
            commands: VecDeque::new(),
            pending: None,
            accepting: true,
            allowed_imports: Vec::new(),
            drain: MclDrain::default(),
            cancel: CancellationToken::new(),
        }
    }
}
impl Component for MclQueue {}
