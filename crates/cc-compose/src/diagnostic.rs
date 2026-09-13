//! Diagnostics : tout ce que l'outil refuse ou signale, avec le chemin dans
//! l'arbre et la position dans le fichier source (CLAUDE.md §1 et phase 1).

use std::fmt;

use crate::compose::node::{Path, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Code stable, en kebab-case, pour les tests et la documentation.
    pub code: &'static str,
    pub message: String,
    pub path: Option<Path>,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic { severity: Severity::Error, code, message: message.into(), path: None, span: None }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic { severity: Severity::Warning, code, message: message.into(), path: None, span: None }
    }

    pub fn at(mut self, path: &Path, span: Span) -> Self {
        self.path = Some(path.clone());
        self.span = Some(span);
        self
    }

    pub fn with_path(mut self, path: &Path) -> Self {
        self.path = Some(path.clone());
        self
    }
}

/// Table des fichiers d'un projet, pour rendre les positions lisibles.
#[derive(Debug, Default, Clone)]
pub struct Files(pub Vec<String>);

impl Files {
    pub fn add(&mut self, name: impl Into<String>) -> usize {
        self.0.push(name.into());
        self.0.len() - 1
    }

    pub fn render(&self, d: &Diagnostic) -> String {
        let mut out = String::new();
        if let Some(span) = d.span {
            let name = self.0.get(span.file).map(String::as_str).unwrap_or("?");
            out.push_str(&format!("{name}:{}:{}: ", span.line, span.col));
        }
        out.push_str(match d.severity {
            Severity::Error => "erreur",
            Severity::Warning => "avertissement",
        });
        out.push_str(&format!(" [{}]", d.code));
        if let Some(p) = &d.path {
            out.push_str(&format!(" {p}"));
        }
        out.push_str(": ");
        out.push_str(&d.message);
        out
    }
}

#[derive(Debug, Default)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}
