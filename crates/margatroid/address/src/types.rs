use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::AddressError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedAddress {
    pub kind: String,
    pub scope: String,
    pub name: String,
    pub tag: String,
}

impl ParsedAddress {
    pub fn parse(value: &str) -> Result<Self, AddressError> {
        let (kind, remainder) = value.split_once(':').ok_or(AddressError::InvalidFormat)?;
        let (scope, name_and_tag) = remainder
            .split_once('/')
            .ok_or(AddressError::InvalidFormat)?;
        if name_and_tag.contains('/') {
            return Err(AddressError::InvalidFormat);
        }
        let (name, tag) = name_and_tag
            .split_once(':')
            .ok_or(AddressError::InvalidFormat)?;
        if kind.is_empty()
            || !kind.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
        {
            return Err(AddressError::InvalidPart);
        }
        for part in [scope, name, tag] {
            if part.is_empty()
                || matches!(part, "." | "..")
                || part.chars().any(|character| {
                    character.is_control() || matches!(character, '/' | '\\' | ':')
                })
            {
                return Err(AddressError::InvalidPart);
            }
        }
        Ok(Self {
            kind: kind.to_owned(),
            scope: scope.to_owned(),
            name: name.to_owned(),
            tag: tag.to_owned(),
        })
    }
}

impl fmt::Display for ParsedAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.kind, self.scope, self.name, self.tag
        )
    }
}

/// File-resource roots only; constructing these paths performs no IO.
#[derive(Clone, Debug)]
pub struct ResourceRoots {
    pub driver: PathBuf,
    pub tool: PathBuf,
    pub prompt: PathBuf,
    pub sandbox: PathBuf,
}
impl ResourceRoots {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        let root = data_root.as_ref();
        Self {
            driver: root.join("agent-drivers"),
            tool: root.join("tools"),
            prompt: root.join("prompts"),
            sandbox: root.join("sandboxes"),
        }
    }
    pub fn root(&self, kind: &str) -> Result<&Path, AddressError> {
        match kind {
            "driver" => Ok(&self.driver),
            "tool" => Ok(&self.tool),
            "prompt" => Ok(&self.prompt),
            "sandbox" => Ok(&self.sandbox),
            _ => Err(AddressError::InvalidPart),
        }
    }
}
impl core_plugin::Resource for ResourceRoots {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceQuery {
    pub resource_type: Option<String>,
    pub scope: Option<String>,
    pub name: Option<String>,
}
impl ResourceQuery {
    pub fn validate(&self) -> Result<(), AddressError> {
        if self
            .resource_type
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "driver" | "tool" | "prompt" | "sandbox"))
        {
            return Err(AddressError::InvalidPart);
        }
        if let Some(scope) = &self.scope {
            ParsedAddress::parse(&format!("tool:{scope}/placeholder:dev"))?;
        }
        if let Some(name) = &self.name {
            ParsedAddress::parse(&format!("tool:placeholder/{name}:dev"))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceFile {
    pub address: ParsedAddress,
    pub path: PathBuf,
    pub tool_path: Option<PathBuf>,
}
