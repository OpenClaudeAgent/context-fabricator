//! Normalized local corpus and atomic checkpoint contract.

pub mod passage;
pub mod snapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind<Text> {
    Current { tracked_ref: Text },
    History,
    Workflow,
}

pub type DocumentKind<'a> = RecordKind<&'a str>;

impl<'a> DocumentKind<'a> {
    pub(crate) const fn parts(self) -> (&'static str, &'a str) {
        match self {
            Self::Current { tracked_ref } => ("current", tracked_ref),
            Self::History => ("history", ""),
            Self::Workflow => ("workflow", ""),
        }
    }
}

pub struct Record<Text> {
    /// Stable provider resource identity, independent of the owning collection.
    pub source: Text,
    /// Immutable source version or workflow run revision.
    pub revision: Text,
    pub kind: RecordKind<Text>,
    pub title: Text,
    pub content: Text,
}

pub type Document<'a> = Record<&'a str>;

impl Record<String> {
    #[must_use]
    pub fn borrowed(&self) -> Document<'_> {
        Document {
            source: &self.source,
            revision: &self.revision,
            kind: match &self.kind {
                RecordKind::Current { tracked_ref } => DocumentKind::Current { tracked_ref },
                RecordKind::History => DocumentKind::History,
                RecordKind::Workflow => DocumentKind::Workflow,
            },
            title: &self.title,
            content: &self.content,
        }
    }
}

pub struct Batch<'a> {
    pub collection: &'a str,
    pub generation: u32,
    pub previous_cursor: Option<&'a str>,
    pub next_cursor: &'a str,
    pub documents: &'a [Document<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionAccess {
    Available,
    Lost,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Checkpoint {
    pub generation: u32,
    pub cursor: Option<String>,
    pub access: CollectionAccess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Commit {
    Indexed,
    AlreadyIndexed,
    StaleCheckpoint,
    AccessLost,
    InvalidProvenance,
}
