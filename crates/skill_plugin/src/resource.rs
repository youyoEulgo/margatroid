use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub allowed_tools: Vec<String>,
    pub preload: bool,
    pub kind: SkillKind,
    pub path: PathBuf,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillKind {
    Member,
    Workflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillRegistryError {
    EmptyName,
    Duplicate(String),
}

impl fmt::Display for SkillRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillRegistryError::EmptyName => write!(f, "skill name cannot be empty"),
            SkillRegistryError::Duplicate(name) => {
                write!(f, "skill `{name}` is already registered")
            }
        }
    }
}

impl std::error::Error for SkillRegistryError {}

pub struct SkillRegistry {
    skills: RwLock<HashMap<String, SkillDescriptor>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, skill: SkillDescriptor) -> Result<(), SkillRegistryError> {
        if skill.name.trim().is_empty() {
            return Err(SkillRegistryError::EmptyName);
        }
        let mut skills = self
            .skills
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if skills.contains_key(&skill.name) {
            return Err(SkillRegistryError::Duplicate(skill.name));
        }
        skills.insert(skill.name.clone(), skill);
        Ok(())
    }

    pub fn replace(&self, skill: SkillDescriptor) -> Result<(), SkillRegistryError> {
        if skill.name.trim().is_empty() {
            return Err(SkillRegistryError::EmptyName);
        }
        self.skills
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(skill.name.clone(), skill);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<SkillDescriptor> {
        self.skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    pub fn list(&self) -> Vec<SkillDescriptor> {
        let mut skills: Vec<_> = self
            .skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LoadedSkills {
    skills: RwLock<HashMap<String, SkillDescriptor>>,
}

impl LoadedSkills {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
        }
    }

    pub fn load(&self, skill: SkillDescriptor) {
        self.skills
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(skill.name.clone(), skill);
    }

    pub fn unload(&self, name: &str) -> bool {
        self.skills
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name)
            .is_some()
    }

    pub fn get(&self, name: &str) -> Option<SkillDescriptor> {
        self.skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    pub fn list(&self) -> Vec<SkillDescriptor> {
        let mut skills: Vec<_> = self
            .skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }
}

impl Default for LoadedSkills {
    fn default() -> Self {
        Self::new()
    }
}
