# skill_plugin

`skill_plugin` provides the V3 skill registry boundary.

## Responsibilities

- Scan skill Markdown files.
- Parse TOML frontmatter.
- Classify member skills and workflow skills.
- Manage skill registry and loaded skill state.

## Public Events

- `SkillScanRequested`
- `SkillScanned`
- `SkillScanFailed`
- `SkillLoadRequested`
- `SkillLoaded`
- `SkillLoadFailed`
- `SkillUnloadRequested`
- `SkillUnloaded`

## Public Resources

- `SkillRegistry`
- `LoadedSkills`

## Stage Registration

- Scan, load, and unload requests are consumed in `Stage::Update`.

## Minimal Example

```rust
use core_plugin::App;
use skill_plugin::{SkillPlugin, SkillScanRequested};

let mut app = App::new();
app.add_plugins(SkillPlugin::new());
app.world().send_event(SkillScanRequested::new("./skills"));
```

## Boundaries

This plugin does not execute workflow DAGs, call LLM providers, or execute sandbox commands.
