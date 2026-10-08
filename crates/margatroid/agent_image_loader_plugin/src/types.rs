use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use margatroid_types::ResourceRef;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::AgentImageLoadError;

#[derive(Clone, Debug)]
pub struct AgentImageBaseDriver {
    source: Arc<str>,
    origin: Arc<PathBuf>,
    source_hash: Arc<str>,
}

impl AgentImageBaseDriver {
    pub(crate) fn new(source: String, origin: PathBuf) -> Self {
        let source_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
        Self {
            source: Arc::from(source),
            origin: Arc::new(origin),
            source_hash: Arc::from(source_hash),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn origin(&self) -> &Path {
        self.origin.as_path()
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }
}

#[cfg(test)]
mod driver_tests {
    use super::AgentImageBaseDriver;

    #[test]
    fn driver_hashes_source_bytes_without_resource_identity_or_compilation() {
        let driver = AgentImageBaseDriver::new("abc".into(), "base.lua".into());
        assert_eq!(driver.source(), "abc");
        assert_eq!(
            driver.source_hash(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let relocated = AgentImageBaseDriver::new("abc".into(), "other/base.lua".into());
        assert_eq!(driver.source_hash(), relocated.source_hash());
        let changed = AgentImageBaseDriver::new("abc\n".into(), "base.lua".into());
        assert_ne!(driver.source_hash(), changed.source_hash());
        assert_eq!(driver.clone().source(), driver.source());
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageDependencies {
    pub(crate) entries: Arc<[ResourceRef]>,
}

impl AgentImageDependencies {
    pub fn entries(&self) -> &[ResourceRef] {
        &self.entries
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageModelParameters {
    pub(crate) temperature: Option<f32>,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) top_p: Option<f32>,
    pub(crate) stop: Arc<[String]>,
}

impl AgentImageModelParameters {
    pub fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    pub fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    pub fn top_p(&self) -> Option<f32> {
        self.top_p
    }

    pub fn stop(&self) -> &[String] {
        &self.stop
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageModelConfig {
    pub(crate) model: Arc<str>,
    pub(crate) parameters: AgentImageModelParameters,
}

impl AgentImageModelConfig {
    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn parameters(&self) -> &AgentImageModelParameters {
        &self.parameters
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageDefaultVisibility {
    pub(crate) resources: BTreeSet<ResourceRef>,
}

impl AgentImageDefaultVisibility {
    pub fn resources(&self) -> impl Iterator<Item = &ResourceRef> + '_ {
        self.resources.iter()
    }
}

#[derive(Deserialize)]
pub(crate) struct AgentImageManifest {
    pub(crate) schema_version: u32,
    pub(crate) inference: AgentImageModelDocument,
    #[serde(default)]
    pub(crate) dependencies: Vec<AgentImageDependencyDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentImageDependencyDocument {
    pub(crate) id: String,
}

#[derive(Deserialize)]
pub(crate) struct AgentImageModelDocument {
    pub(crate) model: String,
    pub(crate) temperature: Option<f32>,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) top_p: Option<f32>,
    #[serde(default)]
    pub(crate) stop: Vec<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct AgentImageLoaderLimits {
    pub(crate) max_manifest_bytes: u64,
    pub(crate) max_model_id_bytes: usize,
    pub(crate) max_stop_sequences: usize,
    pub(crate) max_stop_sequence_bytes: usize,
}

impl Default for AgentImageLoaderLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 64 * 1024,
            max_model_id_bytes: 1024,
            max_stop_sequences: 128,
            max_stop_sequence_bytes: 4096,
        }
    }
}

pub(crate) struct PreparedAgentImage {
    pub(crate) reference: ResourceRef,
    pub(crate) base_driver: AgentImageBaseDriver,
    pub(crate) dependencies: AgentImageDependencies,
    pub(crate) model: AgentImageModelConfig,
    pub(crate) default_visibility: AgentImageDefaultVisibility,
}

pub(crate) struct AgentImageReadPayload {
    pub(crate) reference: ResourceRef,
    pub(crate) result: Result<PreparedAgentImage, AgentImageLoadError>,
}

pub(crate) struct AgentImageReadOutput {
    payload: Mutex<Option<AgentImageReadPayload>>,
}

impl AgentImageReadOutput {
    pub(crate) fn new(payload: AgentImageReadPayload) -> Self {
        Self {
            payload: Mutex::new(Some(payload)),
        }
    }

    pub(crate) fn take(&self) -> Option<AgentImageReadPayload> {
        self.payload
            .lock()
            .expect("agent image read output lock poisoned")
            .take()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryEntryKind {
    File,
    Directory,
}

#[derive(PartialEq, Eq)]
pub(crate) struct DirectoryEntrySignature {
    pub(crate) name: OsString,
    pub(crate) kind: DirectoryEntryKind,
}

#[derive(PartialEq, Eq)]
pub(crate) struct DirectorySignature {
    pub(crate) entries: Vec<DirectoryEntrySignature>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileSignature {
    pub(crate) length: u64,
    pub(crate) modified: Option<SystemTime>,
}
