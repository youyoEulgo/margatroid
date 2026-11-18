use crate::{MclCommand, MclDrain};
use core_plugin::Component;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{atomic::AtomicBool, Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct MclQueue {
    pub commands: VecDeque<MclCommand>,
    pub locked: Arc<AtomicBool>,
    pub active: Arc<Mutex<BTreeMap<String, crate::MclReply>>>,
    pub accepting: bool,
    pub drain: MclDrain,
    pub cancel: CancellationToken,
}
impl Default for MclQueue {
    fn default() -> Self {
        Self {
            commands: VecDeque::new(),
            locked: Arc::new(AtomicBool::new(false)),
            active: Arc::default(),
            accepting: true,
            drain: MclDrain::default(),
            cancel: CancellationToken::new(),
        }
    }
}
impl Component for MclQueue {}
