use std::collections::BTreeSet;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use margatroid_types::ResourceId;
use mcl_plugin::MclProgram;
use serde::Deserialize;

use crate::error::AgentImageLoadError;

#[derive(Clone, Debug)]
pub struct AgentImageBaseDriver {
    pub(crate) program: Arc<MclProgram>,
}

impl AgentImageBaseDriver {
    pub fn program(&self) -> &Arc<MclProgram> {
        &self.program
    }
}

pub type AgentImageBaseMcl = AgentImageBaseDriver;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentImageDependency {
    pub(crate) resource_id: ResourceId,
    pub(crate) source: Option<Arc<str>>,
}

impl AgentImageDependency {
    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Clone, Debug)]
pub struct AgentImageDependencies {
    pub(crate) entries: Arc<[AgentImageDependency]>,
}

impl AgentImageDependencies {
    pub fn entries(&self) -> &[AgentImageDependency] {
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
    pub(crate) resources: BTreeSet<ResourceId>,
}

impl AgentImageDefaultVisibility {
    pub fn resources(&self) -> impl Iterator<Item = &ResourceId> + '_ {
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
pub(crate) struct AgentImageDependencyDocument {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) source: Option<String>,
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
    pub(crate) reference: ResourceId,
    pub(crate) base_driver: AgentImageBaseDriver,
    pub(crate) dependencies: AgentImageDependencies,
    pub(crate) model: AgentImageModelConfig,
    pub(crate) default_visibility: AgentImageDefaultVisibility,
}

pub(crate) struct AgentImageReadPayload {
    pub(crate) reference: ResourceId,
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
