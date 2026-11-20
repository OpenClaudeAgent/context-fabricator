use serde::Serialize;

/// Authority and publication predecessor captured before any outbound read.
#[derive(Clone)]
pub struct Scope {
    pub repository_id: i64,
    pub full_name: String,
    pub branch: String,
    pub collection: String,
    pub generation: u32,
    pub credential: String,
    pub predecessor: Option<i64>,
}

pub struct Pin {
    pub revision: String,
    pub tree: String,
}

pub enum Entry {
    Tree { path: String, oid: String },
    File(SourceFile),
    Skipped,
}

pub struct SourceFile {
    pub path: String,
    pub oid: String,
    pub bytes: u32,
}

#[derive(Serialize)]
pub struct Published {
    pub commit: String,
    pub changed: bool,
}

pub struct Due {
    pub repository_id: i64,
    pub collection: String,
    pub generation: u32,
    pub at_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStatus {
    pub phase: &'static str,
    pub can_refresh: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
