//! Bounded navigation in one explicitly selected Rust crate of an immutable manifest.
//! The caller binds the projection to the repository, ref and commit that supplied the files.
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use serde::Serialize;
use sha1::{Digest, Sha1};

mod parse;
mod resolve;
#[cfg(test)]
mod tests;

/// Rebuild the projection when the grammar, extraction or resolution semantics change.
pub const PROFILE: &str = "rust-tree-sitter-0.24.0-navigation-v2";
pub(crate) const MAX_FILES: usize = 4096;
pub(crate) const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 65_536;

/// One UTF-8 file from the selected manifest. Blob is the Git object ID, not the raw SHA-1.
pub struct Source<'a> {
    pub path: &'a str,
    pub blob: String,
    pub text: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub path: String,
    pub blob: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Coverage {
    Parsed,
    UnsupportedLanguage,
    SyntaxError,
    Limit,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCoverage {
    pub path: String,
    pub coverage: Coverage,
    /// Reachable by unconditional, supported module declarations from this crate root.
    pub reachable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Import,
    Call,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    Resolved,
    Unresolved,
    Ambiguous,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub name: String,
    pub qualified_name: String,
    pub site: Span,
}

#[derive(Debug, Serialize)]
pub struct Reference {
    pub name: String,
    pub site: Span,
    pub role: Role,
    pub resolution: Resolution,
    pub target: Option<Span>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    pub profile: &'static str,
    pub crate_root: String,
    pub files: Vec<FileCoverage>,
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("The manifest path, crate root or Git blob identity is invalid.")]
    InvalidSource,
    #[error("The graph exceeds the bounded snapshot projection.")]
    Limit,
    #[error("The Rust syntax grammar could not be loaded.")]
    Parser,
}

#[derive(Clone)]
struct Token {
    name: String,
    start: usize,
    end: usize,
    line: usize,
}

impl Token {
    fn span(&self, source: &Source<'_>) -> Span {
        Span {
            path: source.path.to_owned(),
            blob: source.blob.clone(),
            start_byte: self.start,
            end_byte: self.end,
            start_line: self.line,
            end_line: self.line,
        }
    }
}

struct Usage {
    token: Token,
    qualified: Option<String>,
    role: Role,
}

#[derive(Clone, Copy)]
enum Visibility {
    Crate,
    Parent,
    Module,
}

struct Declaration {
    usable: bool,
    visibility: Visibility,
}

#[derive(Default)]
struct Facts {
    modules: BTreeMap<String, Vec<Declaration>>,
    functions: Vec<(Token, Declaration)>,
    uses: Vec<Usage>,
}

struct Parsed {
    coverage: Coverage,
    facts: Facts,
}

impl Parsed {
    fn empty(coverage: Coverage) -> Self {
        Self {
            coverage,
            facts: Facts::default(),
        }
    }
}

fn rust_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext == "rs")
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn blob_id(text: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", text.len()));
    hash.update(text);
    let mut id = String::with_capacity(40);
    for byte in hash.finalize() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

/// Extract free functions, direct calls and simple imports from a sealed source manifest.
/// Only absolute `crate::` paths through explicit, unconditional modules can resolve.
/// A separate call for each crate root keeps library and binary target namespaces distinct.
///
/// # Errors
/// Rejects duplicate/invalid paths, forged blobs, a missing root, or snapshot-wide limits.
pub fn project(files: &[Source<'_>], root: &str) -> Result<Projection, Error> {
    project_roots(files, &[root])?
        .pop()
        .ok_or(Error::InvalidSource)
}

/// Parse each unique blob once and resolve the requested crate roots independently.
///
/// # Errors
/// Rejects invalid manifests and work beyond eight roots or the shared output budget.
pub fn project_roots(files: &[Source<'_>], roots: &[&str]) -> Result<Vec<Projection>, Error> {
    if files.len() > MAX_FILES || roots.len() > 8 {
        return Err(Error::Limit);
    }
    if roots.iter().any(|root| !rust_path(root)) {
        return Err(Error::InvalidSource);
    }
    let mut paths = BTreeMap::new();
    let mut parsed = BTreeMap::new();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|_| Error::Parser)?;
    let mut total = 0usize;
    let mut records = 0usize;
    for source in files {
        if !valid_path(source.path) || paths.insert(source.path, source).is_some() {
            return Err(Error::InvalidSource);
        }
        total = total.saturating_add(source.text.len());
        if total > MAX_BYTES {
            return Err(Error::Limit);
        }
        if blob_id(source.text) != source.blob {
            return Err(Error::InvalidSource);
        }
        if rust_path(source.path) {
            let data = parsed
                .entry(source.blob.as_str())
                .or_insert_with(|| parse::extract(&mut parser, source.text));
            // Charge each owning path, including multiple paths to the same immutable bytes.
            records +=
                data.facts.functions.len() + data.facts.uses.len() + data.facts.modules.len();
            if records > MAX_RECORDS {
                return Err(Error::Limit);
            }
        }
    }
    let mut output = Vec::new();
    let mut total_records = 0;
    for &root in roots {
        if !paths.contains_key(root) {
            return Err(Error::InvalidSource);
        }
        let mut result = Projection {
            profile: PROFILE,
            crate_root: root.to_owned(),
            files: Vec::new(),
            definitions: Vec::new(),
            references: Vec::new(),
        };
        let reachable = resolve::connect(root, &paths, &parsed, &mut result)?;
        for (&path, &source) in &paths {
            let coverage = if rust_path(path) {
                parsed[source.blob.as_str()].coverage
            } else {
                Coverage::UnsupportedLanguage
            };
            result.files.push(FileCoverage {
                path: path.to_owned(),
                coverage,
                reachable: reachable.contains(path),
            });
        }
        total_records += result.definitions.len() + result.references.len();
        if total_records > MAX_RECORDS {
            return Err(Error::Limit);
        }
        output.push(result);
    }
    Ok(output)
}
