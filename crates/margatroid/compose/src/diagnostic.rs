use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticCode {
    Io,
    InvalidYaml,
    UnknownField,
    UnsupportedSchema,
    InvalidIdentifier,
    MissingManager,
    DuplicateName,
    UnknownReference,
    InvalidPath,
    PathEscapesProject,
    InvalidResource,
    DigestMismatch,
    ComposeTooLarge,
    ResourceTooLarge,
    BundleTooLarge,
    TooManyResources,
    TooManyFiles,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeDiagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub location: Option<SourceLocation>,
}

impl ComposeDiagnostic {
    pub(crate) fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            location: None,
        }
    }

    pub(crate) fn at_field(mut self, field: impl Into<String>) -> Self {
        self.location = Some(SourceLocation {
            file: PathBuf::new(),
            line: None,
            column: None,
            field: Some(field.into()),
        });
        self
    }

    pub(crate) fn at_position(mut self, line: usize, column: usize) -> Self {
        self.location = Some(SourceLocation {
            file: PathBuf::new(),
            line: u32::try_from(line).ok(),
            column: u32::try_from(column).ok(),
            field: None,
        });
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeCompileError {
    diagnostics: Vec<ComposeDiagnostic>,
}

impl ComposeCompileError {
    pub(crate) fn one(diagnostic: ComposeDiagnostic) -> Self {
        Self {
            diagnostics: vec![diagnostic],
        }
    }

    pub(crate) fn many(diagnostics: Vec<ComposeDiagnostic>) -> Self {
        Self { diagnostics }
    }

    pub fn diagnostics(&self) -> &[ComposeDiagnostic] {
        &self.diagnostics
    }

    pub(crate) fn with_source_file(mut self, file: PathBuf) -> Self {
        for diagnostic in &mut self.diagnostics {
            match &mut diagnostic.location {
                Some(location) if location.file.as_os_str().is_empty() => {
                    location.file = file.clone();
                }
                None => {
                    diagnostic.location = Some(SourceLocation {
                        file: file.clone(),
                        line: None,
                        column: None,
                        field: None,
                    });
                }
                Some(_) => {}
            }
        }
        self
    }

    pub(crate) fn with_field(mut self, field: impl Into<String>) -> Self {
        let field = field.into();
        for diagnostic in &mut self.diagnostics {
            match &mut diagnostic.location {
                Some(location) if location.field.is_none() => {
                    location.field = Some(field.clone());
                }
                None => {
                    diagnostic.location = Some(SourceLocation {
                        file: PathBuf::new(),
                        line: None,
                        column: None,
                        field: Some(field.clone()),
                    });
                }
                Some(_) => {}
            }
        }
        self
    }
}

impl fmt::Display for ComposeCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.diagnostics.is_empty() {
            return formatter.write_str("compose compilation failed");
        }
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{:?}", diagnostic.code)?;
            if let Some(location) = &diagnostic.location {
                write!(formatter, " at {}", location.file.display())?;
                if let Some(line) = location.line {
                    write!(formatter, ":{line}")?;
                    if let Some(column) = location.column {
                        write!(formatter, ":{column}")?;
                    }
                }
                if let Some(field) = &location.field {
                    write!(formatter, " ({field})")?;
                }
            }
            write!(formatter, ": {}", diagnostic.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ComposeCompileError {}

impl From<ComposeDiagnostic> for ComposeCompileError {
    fn from(value: ComposeDiagnostic) -> Self {
        Self::one(value)
    }
}
