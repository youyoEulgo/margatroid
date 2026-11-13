use crate::{ClientError, ClientFrame, ClientReceiver, ClientSender, ConnectionId};
use core_plugin::{Entity, Resource};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientIdentity {
    pub client: Entity,
    pub address: String,
    pub client_type: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientSelector {
    Broadcast,
    Type(String),
    Name(String),
}
#[derive(Clone, Default)]
pub struct Clients {
    inner: Arc<ClientSlots>,
}
#[derive(Default)]
struct ClientSlots {
    registry: RwLock<Registry>,
    sources: Mutex<BTreeMap<crate::SyncKey, crate::SyncSource>>,
}
#[derive(Default)]
struct Registry {
    entries: BTreeMap<ConnectionId, ClientSlot>,
    names: BTreeMap<String, ConnectionId>,
}
struct ClientSlot {
    sender: ClientSender,
    identity: Option<Arc<ClientIdentity>>,
    active: Arc<Mutex<BTreeSet<String>>>,
}
impl Resource for Clients {}
impl std::fmt::Debug for Clients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clients")
            .field(
                "connections",
                &self.inner.registry.read().unwrap().entries.len(),
            )
            .finish()
    }
}
pub struct RequestTicket {
    pub id: String,
    pub connection: ConnectionId,
    active: Arc<Mutex<BTreeSet<String>>>,
}
impl Drop for RequestTicket {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
impl Clients {
    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
    pub fn allocate(&self) -> Option<(ConnectionId, ClientReceiver, CancellationToken)> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = ConnectionId::new(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
                .ok()?,
        );
        let (sender, receiver) = ClientSender::channel(128);
        let cancel = sender.cancel_token();
        self.adopt(id, sender).ok()?;
        Some((id, receiver, cancel))
    }
    pub fn adopt(&self, id: ConnectionId, sender: ClientSender) -> Result<(), ClientError> {
        let mut registry = self.inner.registry.write().unwrap();
        if registry.entries.contains_key(&id) {
            return Err(ClientError::DuplicateClient);
        }
        registry.entries.insert(
            id,
            ClientSlot {
                sender,
                identity: None,
                active: Arc::default(),
            },
        );
        Ok(())
    }
    pub fn get(&self, id: ConnectionId) -> Option<ClientSender> {
        self.inner
            .registry
            .read()
            .unwrap()
            .entries
            .get(&id)
            .map(|v| v.sender.clone())
    }
    pub fn registered(&self, id: ConnectionId) -> Option<Arc<ClientIdentity>> {
        self.inner
            .registry
            .read()
            .unwrap()
            .entries
            .get(&id)
            .and_then(|v| v.identity.clone())
    }
    pub fn commit_registration(
        &self,
        id: ConnectionId,
        identity: Arc<ClientIdentity>,
    ) -> Result<(), ClientError> {
        let mut registry = self.inner.registry.write().unwrap();
        let entry = registry
            .entries
            .get(&id)
            .ok_or(ClientError::ConnectionMissing)?;
        if entry.sender.is_closed() {
            return Err(ClientError::ConnectionMissing);
        }
        if entry.identity.is_some() {
            return Err(ClientError::DuplicateClient);
        }
        if registry.names.contains_key(&identity.name) {
            return Err(ClientError::DuplicateName);
        }
        registry.names.insert(identity.name.clone(), id);
        registry.entries.get_mut(&id).unwrap().identity = Some(identity);
        Ok(())
    }
    pub fn claim(&self, id: ConnectionId, request: &str) -> Option<Arc<RequestTicket>> {
        let registry = self.inner.registry.read().unwrap();
        let entry = registry.entries.get(&id)?;
        if !entry.active.lock().unwrap().insert(request.to_owned()) {
            return None;
        }
        Some(Arc::new(RequestTicket {
            id: request.into(),
            connection: id,
            active: entry.active.clone(),
        }))
    }
    pub fn cancel(&self, id: ConnectionId) {
        if let Some(sender) = self.get(id) {
            sender.cancel();
        }
    }
    pub fn remove(&self, id: ConnectionId) -> bool {
        let mut registry = self.inner.registry.write().unwrap();
        let Some(entry) = registry.entries.remove(&id) else {
            return false;
        };
        if let Some(identity) = entry.identity {
            registry.names.remove(&identity.name);
        }
        entry.sender.cancel();
        true
    }
    pub fn select(&self, targets: &[ClientSelector]) -> Vec<(ConnectionId, ClientSender)> {
        self.inner
            .registry
            .read()
            .unwrap()
            .entries
            .iter()
            .filter_map(|(id, slot)| {
                let identity = slot.identity.as_ref()?;
                let matches = targets.iter().any(|target| match target {
                    ClientSelector::Broadcast => true,
                    ClientSelector::Type(t) => *t == identity.client_type,
                    ClientSelector::Name(n) => *n == identity.name,
                });
                (matches && !slot.sender.is_closed()).then(|| (*id, slot.sender.clone()))
            })
            .collect()
    }
    pub fn initialize(&self, id: ConnectionId) {
        let sources = self
            .inner
            .sources
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for source in sources {
            if source.matches(self, id) {
                if let Some(sender) = self.get(id) {
                    let _ = sender.try_send_state(source);
                }
            }
        }
    }
    pub fn register_source(
        &self,
        key: crate::SyncKey,
        targets: Arc<[ClientSelector]>,
        prepare: crate::StatePrepare,
    ) -> crate::SyncSource {
        let source = crate::SyncSource::new(key.clone(), targets, prepare);
        if let Some(old) = self
            .inner
            .sources
            .lock()
            .unwrap()
            .insert(key, source.clone())
        {
            old.close();
        }
        for (_, sender) in self.select(source.targets()) {
            let _ = sender.try_send_state(source.clone());
        }
        source
    }
    pub fn publish_state(&self, source: &crate::SyncSource, revision: u64) {
        if !source.advance(revision) {
            return;
        }
        for (_, sender) in self.select(source.targets()) {
            let _ = sender.try_send_state(source.clone());
        }
    }
    pub fn retire_source(&self, source: &crate::SyncSource) {
        source.close();
        let mut sources = self.inner.sources.lock().unwrap();
        if sources
            .get(source.key())
            .is_some_and(|current| current.epoch() == source.epoch())
        {
            sources.remove(source.key());
        }
    }
    pub fn send_scoped(
        &self,
        targets: &[ClientSelector],
        frame: Arc<ClientFrame>,
        active: Arc<std::sync::atomic::AtomicBool>,
    ) {
        if !active.load(Ordering::Acquire) {
            return;
        }
        for (_, sender) in self.select(targets) {
            let _ = sender.try_send_scoped(frame.clone(), active.clone());
        }
    }
    pub fn send_ordered(&self, targets: &[ClientSelector], frame: Arc<ClientFrame>) {
        for (_, sender) in self.select(targets) {
            let _ = sender.try_send_shared(frame.clone());
        }
    }
}
