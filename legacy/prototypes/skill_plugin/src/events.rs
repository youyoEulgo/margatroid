use std::path::PathBuf;

use crate::resource::SkillDescriptor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillScanRequested {
    pub root: PathBuf,
}

impl SkillScanRequested {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[derive(Clone, Debug)]
pub struct SkillScanned {
    pub root: PathBuf,
    pub skills: Vec<SkillDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillScanFailed {
    pub root: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillLoadRequested {
    pub name: String,
}

impl SkillLoadRequested {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Debug)]
pub struct SkillLoaded {
    pub skill: SkillDescriptor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillLoadFailed {
    pub name: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillUnloadRequested {
    pub name: String,
}

impl SkillUnloadRequested {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillUnloaded {
    pub name: String,
}
