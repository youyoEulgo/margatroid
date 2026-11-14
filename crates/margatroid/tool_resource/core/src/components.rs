use std::collections::{BTreeMap, BTreeSet};
use std::path::Component as PathComponent;

use core_plugin::Component;

use crate::{
    validate_alias, validate_schema, ToolEntry, ToolResourceError, ToolResourceErrorKind as Kind,
};

#[derive(Debug)]
pub struct Tools {
    entries: BTreeMap<String, ToolEntry>,
    registrations: BTreeMap<String, BoundRegistration>,
    updates: client_core::UpdateProbe<Vec<(String, ToolEntry)>>,
}

impl Tools {
    pub fn new(updates: client_core::UpdateProbe<Vec<(String, ToolEntry)>>) -> Self {
        Self {
            entries: BTreeMap::new(),
            registrations: BTreeMap::new(),
            updates,
        }
    }
    pub fn publish_current(&self) {
        self.updates.publish(
            self.iter()
                .map(|(alias, entry)| (alias.clone(), entry.clone()))
                .collect(),
        );
    }

    pub fn get(&self, alias: &str) -> Option<&ToolEntry> {
        if self
            .registrations
            .get(alias)
            .is_some_and(|registration| !(registration.available)())
        {
            return None;
        }
        self.entries.get(alias)
    }

    pub fn insert(&mut self, alias: String, entry: ToolEntry) -> Result<(), ToolResourceError> {
        self.insert_batch(vec![(alias, entry)])
    }

    pub fn insert_batch(
        &mut self,
        entries: Vec<(String, ToolEntry)>,
    ) -> Result<(), ToolResourceError> {
        self.insert_registered_batch(entries, Vec::new())
    }

    pub fn insert_registered_batch(
        &mut self,
        entries: Vec<(String, ToolEntry)>,
        registrations: Vec<RegistrationBinding>,
    ) -> Result<(), ToolResourceError> {
        let mut aliases = BTreeSet::new();
        for (alias, entry) in &entries {
            validate_alias(alias)?;
            if self.entries.contains_key(alias) || !aliases.insert(alias) {
                return Err(ToolResourceError::new(
                    Kind::AliasConflict,
                    format!("alias {alias} is already registered"),
                )
                .with_path(&entry.path));
            }
            if !entry.path.is_absolute()
                || entry
                    .path
                    .components()
                    .any(|part| matches!(part, PathComponent::ParentDir))
            {
                return Err(ToolResourceError::new(
                    Kind::InvalidPath,
                    "entry path must be absolute without parent traversal",
                )
                .with_path(&entry.path));
            }
            if entry.description.trim().is_empty() {
                return Err(ToolResourceError::new(
                    Kind::InvalidDefinition,
                    "description must be nonempty",
                )
                .with_path(&entry.path));
            }
            validate_schema(&entry.input_schema).map_err(|error| error.with_path(&entry.path))?;
        }
        if entries.is_empty() {
            return Ok(());
        }
        if registrations
            .iter()
            .any(|(_, _, _, available)| !available())
        {
            return Err(ToolResourceError::new(
                Kind::InvalidDefinition,
                "lifetime task closed during registration",
            ));
        }
        self.entries.extend(entries);
        self.registrations.extend(registrations.into_iter().map(
            |(alias, id, guard, available)| {
                (
                    alias,
                    BoundRegistration {
                        id,
                        _guard: guard,
                        available,
                    },
                )
            },
        ));
        self.publish_current();
        Ok(())
    }
    pub fn remove(&mut self, alias: &str) -> Option<ToolEntry> {
        self.registrations.remove(alias);
        let entry = self.entries.remove(alias);
        if entry.is_some() {
            self.publish_current();
        }
        entry
    }
    pub fn revoke(&mut self, alias: &str, id: RegistrationId) {
        if self
            .registrations
            .get(alias)
            .is_some_and(|binding| binding.id == id)
        {
            self.registrations.remove(alias);
            self.entries.remove(alias);
            self.publish_current();
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ToolEntry)> {
        self.entries.iter().filter(|(alias, _)| {
            self.registrations
                .get(*alias)
                .is_none_or(|registration| (registration.available)())
        })
    }
}

impl Component for Tools {}

impl PartialEq for Tools {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}
impl Eq for Tools {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrationId(u64);
impl RegistrationId {
    pub fn fresh() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}
pub type RegistrationBinding = (
    String,
    RegistrationId,
    Box<dyn Send + Sync>,
    Box<dyn Fn() -> bool + Send + Sync>,
);
struct BoundRegistration {
    available: Box<dyn Fn() -> bool + Send + Sync>,
    id: RegistrationId,
    _guard: Box<dyn Send + Sync>,
}
impl std::fmt::Debug for BoundRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundRegistration")
            .field("id", &self.id)
            .finish()
    }
}
