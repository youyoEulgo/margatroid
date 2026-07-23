# margatroid_compose

`margatroid_compose` is the local compiler for `margatroid-workspace.yaml` projects.

It parses and validates the authoring YAML, resolves project-level and main-directory Skill and
Workflow packages, and produces a deterministic `margatroid_protocol::WorkspaceBundle`.

The crate does not start ECS, connect to the daemon, execute workflows, or read Provider secrets.
