//! Local compiler for `margatroid-workspace.yaml` projects.

mod compiler;
mod diagnostic;
mod document;
mod package;

pub use compiler::{
    CompileOptions, CompileOutput, Compiler, NormalizedProject, ProjectLimits, RenderError, compile,
};
pub use diagnostic::{ComposeCompileError, ComposeDiagnostic, DiagnosticCode, SourceLocation};

pub const DEFAULT_COMPOSE_FILE: &str = "margatroid-workspace.yaml";
pub const ALTERNATE_COMPOSE_FILE: &str = "margatroid-workspace.yml";
