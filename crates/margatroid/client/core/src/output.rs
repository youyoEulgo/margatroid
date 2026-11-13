use crate::{ClientFrame, ClientSelector, Clients};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SyncKey {
    pub topic: String,
    pub object: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientPrepareError {
    SourceClosed,
    EncodeFailed,
    TooLarge,
    ReadFailed,
}
pub struct PreparedState {
    pub revision: u64,
    pub frame: Arc<ClientFrame>,
}
pub type PrepareFuture =
    Pin<Box<dyn Future<Output = Result<PreparedState, ClientPrepareError>> + Send>>;
pub type StatePrepare = Arc<dyn Fn() -> PrepareFuture + Send + Sync>;
#[derive(Clone)]
pub struct SyncSource(Arc<StateSource>);
struct StateSource {
    key: SyncKey,
    epoch: u64,
    active: AtomicBool,
    revision: AtomicU64,
    targets: Arc<[ClientSelector]>,
    prepare: StatePrepare,
}
impl SyncSource {
    pub(crate) fn new(key: SyncKey, targets: Arc<[ClientSelector]>, prepare: StatePrepare) -> Self {
        static EPOCH: AtomicU64 = AtomicU64::new(1);
        Self(Arc::new(StateSource {
            key,
            epoch: EPOCH.fetch_add(1, Ordering::Relaxed),
            active: AtomicBool::new(true),
            revision: AtomicU64::new(0),
            targets,
            prepare,
        }))
    }
    pub fn key(&self) -> &SyncKey {
        &self.0.key
    }
    pub fn epoch(&self) -> u64 {
        self.0.epoch
    }
    pub fn is_active(&self) -> bool {
        self.0.active.load(Ordering::Acquire)
    }
    pub fn close(&self) {
        self.0.active.store(false, Ordering::Release);
    }
    pub(crate) fn targets(&self) -> &[ClientSelector] {
        &self.0.targets
    }
    pub(crate) fn advance(&self, revision: u64) -> bool {
        self.is_active()
            && self
                .0
                .revision
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |previous| {
                    (revision > previous).then_some(revision)
                })
                .is_ok()
    }
    pub(crate) fn matches(&self, clients: &Clients, id: crate::ConnectionId) -> bool {
        self.is_active()
            && clients
                .select(&self.0.targets)
                .iter()
                .any(|(current, _)| *current == id)
    }
}
impl std::fmt::Debug for SyncSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncSource")
            .field("key", self.key())
            .field("epoch", &self.epoch())
            .finish()
    }
}
#[derive(Clone)]
pub struct ClientSender {
    sender: mpsc::Sender<OutputItem>,
    output: Arc<ClientOutput>,
}
struct ClientOutput {
    pending: Mutex<BTreeMap<SyncKey, SyncSource>>,
    cancel: CancellationToken,
}
enum OutputItem {
    Ordered {
        frame: Arc<ClientFrame>,
        active: Option<Arc<AtomicBool>>,
    },
    State(SyncKey),
}
pub struct ClientReceiver {
    receiver: mpsc::Receiver<OutputItem>,
    output: Arc<ClientOutput>,
    preparing: Option<(SyncSource, PrepareFuture)>,
    sent: BTreeMap<SyncKey, (u64, u64)>,
}
impl ClientSender {
    pub fn channel(capacity: usize) -> (Self, ClientReceiver) {
        let (sender, receiver) = mpsc::channel(capacity);
        let output = Arc::new(ClientOutput {
            pending: Mutex::default(),
            cancel: CancellationToken::new(),
        });
        (
            Self {
                sender,
                output: output.clone(),
            },
            ClientReceiver {
                receiver,
                output,
                preparing: None,
                sent: BTreeMap::new(),
            },
        )
    }
    pub fn try_send(&self, frame: ClientFrame) -> Result<(), crate::ClientSendError> {
        self.try_send_shared(Arc::new(frame))
    }
    pub fn try_send_shared(&self, frame: Arc<ClientFrame>) -> Result<(), crate::ClientSendError> {
        self.enqueue(OutputItem::Ordered {
            frame,
            active: None,
        })
    }
    pub fn try_send_scoped(
        &self,
        frame: Arc<ClientFrame>,
        active: Arc<AtomicBool>,
    ) -> Result<(), crate::ClientSendError> {
        self.enqueue(OutputItem::Ordered {
            frame,
            active: Some(active),
        })
    }
    fn enqueue(&self, item: OutputItem) -> Result<(), crate::ClientSendError> {
        if self.output.cancel.is_cancelled() {
            return Err(crate::ClientSendError::Closed);
        }
        self.sender.try_send(item).map_err(|error| {
            self.output.cancel.cancel();
            match error {
                mpsc::error::TrySendError::Full(_) => crate::ClientSendError::Full,
                mpsc::error::TrySendError::Closed(_) => crate::ClientSendError::Closed,
            }
        })
    }
    pub fn try_send_state(&self, source: SyncSource) -> Result<(), crate::ClientSendError> {
        if !source.is_active() {
            return Ok(());
        }
        let mut pending = self.output.pending.lock().unwrap();
        let key = source.key().clone();
        if let Some(old) = pending.get_mut(&key) {
            if old.epoch() <= source.epoch() {
                *old = source;
            }
            return Ok(());
        }
        pending.insert(key.clone(), source);
        if let Err(error) = self.enqueue(OutputItem::State(key.clone())) {
            pending.remove(&key);
            return Err(error);
        }
        Ok(())
    }
    pub fn cancel(&self) {
        self.output.cancel.cancel();
    }
    pub fn cancel_token(&self) -> CancellationToken {
        self.output.cancel.clone()
    }
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed() || self.output.cancel.is_cancelled()
    }
}
impl std::fmt::Debug for ClientSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSender")
            .field("closed", &self.is_closed())
            .finish()
    }
}
impl ClientReceiver {
    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<ClientFrame>> {
        loop {
            if self.output.cancel.is_cancelled() && self.preparing.is_none() {
                self.receiver.close();
                return Poll::Ready(None);
            }
            if let Some((source, future)) = self.preparing.as_mut() {
                let result = match future.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(value) => value,
                };
                let source = source.clone();
                self.preparing = None;
                if !source.is_active() || self.output.cancel.is_cancelled() {
                    continue;
                }
                match result {
                    Ok(value) => {
                        if self
                            .sent
                            .get(source.key())
                            .is_some_and(|(epoch, revision)| {
                                *epoch > source.epoch()
                                    || (*epoch == source.epoch() && *revision >= value.revision)
                            })
                        {
                            continue;
                        }
                        self.sent
                            .insert(source.key().clone(), (source.epoch(), value.revision));
                        return Poll::Ready(Some((*value.frame).clone()));
                    }
                    Err(ClientPrepareError::SourceClosed) => continue,
                    Err(error) => {
                        return Poll::Ready(Some(ClientFrame::Close {
                            code: if error == ClientPrepareError::TooLarge {
                                1009
                            } else {
                                1011
                            },
                            reason: "sync preparation failed".into(),
                        }))
                    }
                }
            }
            let item = match self.receiver.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(value)) => value,
            };
            match item {
                OutputItem::Ordered { frame, active } => {
                    if active
                        .as_ref()
                        .is_none_or(|flag| flag.load(Ordering::Acquire))
                    {
                        return Poll::Ready(Some((*frame).clone()));
                    }
                }
                OutputItem::State(key) => {
                    let source = self.output.pending.lock().unwrap().remove(&key);
                    if let Some(source) = source.filter(SyncSource::is_active) {
                        let future = (source.0.prepare)();
                        self.preparing = Some((source, future));
                    }
                }
            }
        }
    }
    pub async fn recv(&mut self) -> Option<ClientFrame> {
        std::future::poll_fn(|cx| self.poll_recv(cx)).await
    }
    pub fn try_recv(&mut self) -> Result<ClientFrame, mpsc::error::TryRecvError> {
        let mut cx = Context::from_waker(std::task::Waker::noop());
        match self.poll_recv(&mut cx) {
            Poll::Ready(Some(frame)) => Ok(frame),
            Poll::Ready(None) => Err(mpsc::error::TryRecvError::Disconnected),
            Poll::Pending => Err(mpsc::error::TryRecvError::Empty),
        }
    }
    pub async fn close(&mut self) {
        self.output.cancel.cancel();
        self.receiver.close();
        self.output.pending.lock().unwrap().clear();
        if let Some((_, future)) = self.preparing.take() {
            let _ = future.await;
        }
    }
}
impl Drop for ClientReceiver {
    fn drop(&mut self) {
        self.output.cancel.cancel();
    }
}

pub struct SyncContext<T> {
    clients: Clients,
    source: SyncSource,
    marker: std::marker::PhantomData<fn() -> T>,
}
impl<T> Clone for SyncContext<T> {
    fn clone(&self) -> Self {
        Self {
            clients: self.clients.clone(),
            source: self.source.clone(),
            marker: std::marker::PhantomData,
        }
    }
}
impl<T: Send + 'static> SyncContext<T> {
    pub fn new(
        clients: Clients,
        key: SyncKey,
        targets: Arc<[ClientSelector]>,
        snapshot: impl Fn() -> Result<(u64, T), ClientPrepareError> + Send + Sync + 'static,
        encode: impl Fn(T) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync + 'static,
    ) -> Self {
        let snapshot = Arc::new(snapshot);
        let encode = Arc::new(encode);
        let prepare: StatePrepare = Arc::new(move || {
            let snapshot = snapshot.clone();
            let encode = encode.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let (revision, value) = snapshot()?;
                    Ok(PreparedState {
                        revision,
                        frame: encode(value)?,
                    })
                })
                .await
                .map_err(|_| ClientPrepareError::ReadFailed)?
            })
        });
        let source = clients.register_source(key, targets, prepare);
        Self {
            clients,
            source,
            marker: std::marker::PhantomData,
        }
    }
    pub fn publish(&self, revision: u64) {
        self.clients.publish_state(&self.source, revision);
    }
    pub fn close(&self) {
        self.clients.retire_source(&self.source);
    }
    pub fn source(&self) -> &SyncSource {
        &self.source
    }
}
