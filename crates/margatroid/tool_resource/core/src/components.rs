use std::collections::{BTreeMap, BTreeSet};
use std::path::Component as PathComponent;

use core_plugin::Component;

use crate::{
    validate_alias, validate_schema, ToolEntry, ToolResourceError, ToolResourceErrorKind as Kind,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tools {
    entries: BTreeMap<String, ToolEntry>,
}

impl Tools {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, alias: &str) -> Option<&ToolEntry> {
        self.entries.get(alias)
    }

    pub fn insert(&mut self, alias: String, entry: ToolEntry) -> Result<(), ToolResourceError> {
        self.insert_batch(vec![(alias, entry)])
    }

    pub fn insert_batch(
        &mut self,
        entries: Vec<(String, ToolEntry)>,
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
        self.entries.extend(entries);
        Ok(())
    }

    pub fn remove_owner(&mut self, owner_task_id: &str) {
        self.entries
            .retain(|_, entry| entry.owner_task_id.as_deref() != Some(owner_task_id));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &ToolEntry)> {
        self.entries.iter()
    }
}

impl Component for Tools {}
