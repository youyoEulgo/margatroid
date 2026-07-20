mod events;
mod plugin;
mod resource;
mod systems;

pub use events::{
    SkillLoadFailed, SkillLoadRequested, SkillLoaded, SkillScanFailed, SkillScanRequested,
    SkillScanned, SkillUnloadRequested, SkillUnloaded,
};
pub use plugin::SkillPlugin;
pub use resource::{LoadedSkills, SkillDescriptor, SkillKind, SkillRegistry, SkillRegistryError};
