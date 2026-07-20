use core_plugin::{App, Plugin, Stage, World};

use crate::events::{
    SkillLoadFailed, SkillLoadRequested, SkillLoaded, SkillScanFailed, SkillScanRequested,
    SkillScanned, SkillUnloadRequested, SkillUnloaded,
};
use crate::resource::{LoadedSkills, SkillRegistry};
use crate::systems::{load_requested_skills, scan_requested_skills, unload_requested_skills};

#[derive(Clone, Debug, Default)]
pub struct SkillPlugin;

impl SkillPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Plugin for SkillPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SkillScanRequested>();
        app.add_event::<SkillScanned>();
        app.add_event::<SkillScanFailed>();
        app.add_event::<SkillLoadRequested>();
        app.add_event::<SkillLoaded>();
        app.add_event::<SkillLoadFailed>();
        app.add_event::<SkillUnloadRequested>();
        app.add_event::<SkillUnloaded>();

        if app.world().resource::<SkillRegistry>().is_none() {
            app.world_mut().add_resource(SkillRegistry::new());
        }
        if app.world().resource::<LoadedSkills>().is_none() {
            app.world_mut().add_resource(LoadedSkills::new());
        }

        let mut scan_reader = app.event_reader::<SkillScanRequested>();
        app.add_systems(
            Stage::Input,
            [move |world: &mut World| {
                scan_requested_skills(world, &mut scan_reader);
            }],
        );

        let mut load_reader = app.event_reader::<SkillLoadRequested>();
        app.add_systems(
            Stage::Prepare,
            [move |world: &mut World| {
                load_requested_skills(world, &mut load_reader);
            }],
        );

        let mut unload_reader = app.event_reader::<SkillUnloadRequested>();
        app.add_systems(
            Stage::Prepare,
            [move |world: &mut World| {
                unload_requested_skills(world, &mut unload_reader);
            }],
        );
    }
}
