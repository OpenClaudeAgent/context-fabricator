use base64::{Engine as _, engine::general_purpose::STANDARD};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use super::{
    GitHubRepository, GitHubRest,
    objects::{Branch, Object, oid, valid_oid},
    request::json,
};
use crate::application::github::{
    GitHubFuture,
    history::{ComparisonSource, Source, SourceRequest, TreeEntry},
};
use crate::domain::{
    github::{
        Error, RepositoryName,
        history::{
            Commit, Pin, Scope,
            comparison::{File, MAX_FILE_BYTES},
        },
    },
    sync::RequestKind,
};

impl ComparisonSource for GitHubRest {
    fn tree<'a>(
        &'a self,
        repository: &'a str,
        id: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Vec<TreeEntry>> {
        Box::pin(async move {
            RepositoryName::parse(repository)?;
            if !valid_oid(id) {
                return Err(Error::Upstream);
            }
            let path = format!("/repos/{repository}/git/trees/{id}");
            let tree: WireTree = self
                .get_metadata(
                    request,
                    RequestKind::Pagination,
                    &path,
                    "/repos/{owner}/{repo}/git/trees/{sha}",
                    json,
                )
                .await?;
            if tree.sha != id {
                return Err(Error::Upstream);
            }
            if tree.truncated || tree.entries.len() > 4096 {
                return Err(Error::CollectionLimit);
            }
            let mut paths = std::collections::HashSet::new();
            if tree.entries.iter().any(|e| !paths.insert(&e.path)) {
                return Err(Error::Upstream);
            }
            tree.entries
                .into_iter()
                .map(|entry| {
                    if entry.path.is_empty()
                        || entry.path.len() > 255
                        || entry.path.contains('/')
                        || matches!(entry.path.as_str(), "." | "..")
                        || entry.path.chars().any(char::is_control)
                    {
                        return Err(Error::Upstream);
                    }
                    let sha = oid(entry.sha)?;
                    match (entry.kind.as_str(), entry.mode.as_str()) {
                        ("tree", "040000") => Ok(TreeEntry::Tree {
                            path: entry.path,
                            id: sha,
                        }),
                        ("blob", "100644" | "100755" | "120000") | ("commit", "160000") => {
                            if entry.kind == "blob" && entry.size.is_none() {
                                return Err(Error::Upstream);
                            }
                            Ok(TreeEntry::File(File {
                                path: entry.path,
                                blob: sha,
                                size: entry.size,
                                mode: entry.mode,
                            }))
                        }
                        _ => Err(Error::Upstream),
                    }
                })
                .collect()
        })
    }

    fn blob<'a>(
        &'a self,
        repository: &'a str,
        id: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Vec<u8>> {
        Box::pin(async move {
            RepositoryName::parse(repository)?;
            if !valid_oid(id) {
                return Err(Error::Upstream);
            }
            let path = format!("/repos/{repository}/git/blobs/{id}");
            let blob: WireBlob = self
                .get_metadata(
                    request,
                    RequestKind::Pagination,
                    &path,
                    "/repos/{owner}/{repo}/git/blobs/{sha}",
                    json,
                )
                .await?;
            if blob.sha != id || blob.encoding != "base64" || blob.size > MAX_FILE_BYTES {
                return Err(Error::Upstream);
            }
            let encoded: String = blob
                .content
                .chars()
                .filter(|c| !matches!(c, '\r' | '\n'))
                .collect();
            let bytes = STANDARD.decode(encoded).map_err(|_| Error::Upstream)?;
            if bytes.len() != blob.size as usize {
                return Err(Error::Upstream);
            }
            Ok(bytes)
        })
    }
}

impl Source for GitHubRest {
    fn verify_repository<'a>(
        &'a self,
        scope: &'a Scope,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, String> {
        Box::pin(async move {
            RepositoryName::parse(&scope.full_name)?;
            let path = format!("/repos/{}", scope.full_name);
            let result: GitHubRepository = self
                .get_metadata(
                    request,
                    RequestKind::Manual,
                    &path,
                    "/repos/{owner}/{repo}",
                    json,
                )
                .await?;
            let verified = result.validated()?;
            if verified.id != scope.repository_id {
                return Err(Error::Conflict);
            }
            Ok(verified.full_name)
        })
    }

    fn pin<'a>(
        &'a self,
        scope: &'a Scope,
        repository: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Pin> {
        Box::pin(async move {
            let branch = utf8_percent_encode(&scope.branch, NON_ALPHANUMERIC);
            let path = format!("/repos/{repository}/branches/{branch}");
            let result: Branch = self
                .get_metadata(
                    request,
                    RequestKind::Manual,
                    &path,
                    "/repos/{owner}/{repo}/branches/{branch}",
                    json,
                )
                .await?;
            let pin = result.pin(&scope.branch)?;
            Ok(Pin {
                commit: pin.revision,
                tree: pin.tree,
            })
        })
    }

    fn commit<'a>(
        &'a self,
        repository: &'a str,
        id: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Commit> {
        Box::pin(async move {
            RepositoryName::parse(repository)?;
            if !valid_oid(id) {
                return Err(Error::Upstream);
            }
            let path = format!("/repos/{repository}/git/commits/{id}");
            let wire: WireCommit = self
                .get_metadata(
                    request,
                    RequestKind::Pagination,
                    &path,
                    "/repos/{owner}/{repo}/git/commits/{sha}",
                    json,
                )
                .await?;
            if wire.sha != id {
                return Err(Error::Upstream);
            }
            if wire.parents.len() > 32 {
                return Err(Error::CollectionLimit);
            }
            Ok(Commit {
                id: wire.sha,
                tree: oid(wire.tree.sha)?,
                parents: wire
                    .parents
                    .into_iter()
                    .map(|parent| oid(parent.sha))
                    .collect::<Result<_, _>>()?,
                summary: wire
                    .message
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(240)
                    .collect(),
            })
        })
    }
}

#[derive(Deserialize)]
struct WireCommit {
    sha: String,
    tree: Object,
    parents: Vec<Object>,
    message: String,
}

#[derive(Deserialize)]
struct WireTree {
    sha: String,
    truncated: bool,
    #[serde(rename = "tree")]
    entries: Vec<WireEntry>,
}
#[derive(Deserialize)]
struct WireEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    size: Option<u64>,
}
#[derive(Deserialize)]
struct WireBlob {
    sha: String,
    size: u32,
    encoding: String,
    content: String,
}
