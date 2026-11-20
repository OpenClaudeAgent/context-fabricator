use std::fmt::Write as _;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use super::{
    GitHubRepository, GitHubRest,
    request::{json, skip_body},
};
use crate::application::github::{GitHubFuture, current::Source};
use crate::domain::github::{
    Error, RepositoryName,
    current::{Entry, Pin, Scope, SourceFile},
};
use crate::domain::sync::RequestKind;

const MAX_FILE_BYTES: u64 = 256 * 1024;

impl Source for GitHubRest {
    fn verify_repository<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
    ) -> GitHubFuture<'a, String> {
        Box::pin(async move {
            RepositoryName::parse(&scope.full_name)?;
            let path = format!("/repos/{}", scope.full_name);
            let repository: GitHubRepository = self
                .get_guarded(secret, RequestKind::Manual, &path, json, Some(scope))
                .await?;
            let repository = repository.validated()?;
            if repository.id != scope.repository_id {
                return Err(Error::Conflict);
            }
            Ok(repository.full_name)
        })
    }

    fn verify_contents<'a>(&'a self, secret: &'a [u8], scope: &'a Scope) -> GitHubFuture<'a, ()> {
        Box::pin(async move {
            let path = format!("/repos/{}/branches?per_page=1", scope.full_name);
            self.get_guarded(
                secret,
                RequestKind::Validation,
                &path,
                skip_body,
                Some(scope),
            )
            .await
        })
    }

    fn branch<'a>(&'a self, secret: &'a [u8], scope: &'a Scope) -> GitHubFuture<'a, Pin> {
        Box::pin(async move {
            let branch = utf8_percent_encode(&scope.branch, NON_ALPHANUMERIC);
            let path = format!("/repos/{}/branches/{branch}", scope.full_name);
            let result: Branch = self
                .get_guarded(secret, RequestKind::Manual, &path, json, Some(scope))
                .await?;
            if result.name != scope.branch {
                return Err(Error::Upstream);
            }
            Ok(Pin {
                revision: oid(result.commit.sha)?,
                tree: oid(result.commit.commit.tree.sha)?,
            })
        })
    }

    fn commit_tree<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
        id: &'a str,
    ) -> GitHubFuture<'a, String> {
        Box::pin(async move {
            let path = object_route(scope, "commits", id)?;
            let result: Commit = self
                .get_guarded(secret, RequestKind::Pagination, &path, json, Some(scope))
                .await?;
            if result.sha != id {
                return Err(Error::Upstream);
            }
            oid(result.tree.sha)
        })
    }

    fn tree<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
        id: &'a str,
    ) -> GitHubFuture<'a, Vec<Entry>> {
        Box::pin(async move {
            let path = object_route(scope, "trees", id)?;
            let mut result: Tree = self
                .get_guarded(secret, RequestKind::Pagination, &path, json, Some(scope))
                .await?;
            if result.sha != id {
                return Err(Error::Upstream);
            }
            if result.truncated {
                return Err(Error::CollectionLimit);
            }
            result
                .entries
                .sort_unstable_by(|left, right| left.path.cmp(&right.path));
            if result
                .entries
                .windows(2)
                .any(|pair| pair[0].path == pair[1].path)
            {
                return Err(Error::Upstream);
            }
            result
                .entries
                .into_iter()
                .map(TreeEntry::validated)
                .collect()
        })
    }

    fn blob<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
        id: &'a str,
        bytes: u32,
    ) -> GitHubFuture<'a, Option<String>> {
        Box::pin(async move {
            let path = object_route(scope, "blobs", id)?;
            let result: Blob = self
                .get_guarded(secret, RequestKind::Pagination, &path, json, Some(scope))
                .await?;
            result.text(id, bytes)
        })
    }
}

#[derive(Deserialize)]
struct Branch {
    name: String,
    commit: BranchCommit,
}

#[derive(Deserialize)]
struct BranchCommit {
    sha: String,
    commit: CommitTree,
}

#[derive(Deserialize)]
struct CommitTree {
    tree: Object,
}

#[derive(Deserialize)]
struct Object {
    sha: String,
}

#[derive(Deserialize)]
struct Commit {
    sha: String,
    tree: Object,
}

#[derive(Deserialize)]
struct Tree {
    sha: String,
    truncated: bool,
    #[serde(rename = "tree")]
    entries: Vec<TreeEntry>,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    size: Option<u64>,
}

impl TreeEntry {
    fn validated(self) -> Result<Entry, Error> {
        if self.path.is_empty()
            || self.path.contains('/')
            || matches!(self.path.as_str(), "." | "..")
            || self.path.chars().any(char::is_control)
        {
            return Err(Error::Upstream);
        }
        match (self.kind.as_str(), self.mode.as_str()) {
            ("tree", "040000") => Ok(Entry::Tree {
                path: self.path,
                oid: oid(self.sha)?,
            }),
            ("blob", "100644" | "100755") => {
                let bytes = self.size.ok_or(Error::Upstream)?;
                if bytes > MAX_FILE_BYTES {
                    return Ok(Entry::Skipped);
                }
                Ok(Entry::File(SourceFile {
                    path: self.path,
                    oid: oid(self.sha)?,
                    bytes: u32::try_from(bytes).map_err(|_| Error::Upstream)?,
                }))
            }
            ("blob", "120000") | ("commit", "160000") => Ok(Entry::Skipped),
            _ => Err(Error::Upstream),
        }
    }
}

#[derive(Deserialize)]
struct Blob {
    sha: String,
    size: u32,
    encoding: String,
    content: String,
}

impl Blob {
    fn text(self, id: &str, expected: u32) -> Result<Option<String>, Error> {
        if self.sha != id || self.size != expected || self.encoding != "base64" {
            return Err(Error::Upstream);
        }
        let encoded: String = self
            .content
            .chars()
            .filter(|c| !matches!(c, '\r' | '\n'))
            .collect();
        let bytes = STANDARD.decode(encoded).map_err(|_| Error::Upstream)?;
        if usize::try_from(expected).ok() != Some(bytes.len()) {
            return Err(Error::Upstream);
        }
        // GitHub's SHA-1 object ID is the wire identity of the Git blob.
        let mut git = Sha1::new();
        git.update(format!("blob {}\0", bytes.len()));
        git.update(&bytes);
        let mut hash = String::with_capacity(40);
        for byte in git.finalize() {
            let _ = write!(hash, "{byte:02x}");
        }
        if hash != id {
            return Err(Error::Upstream);
        }
        Ok(String::from_utf8(bytes).ok().filter(|text| {
            !text.contains('\0')
                && !text.starts_with("version https://git-lfs.github.com/spec/v1\n")
        }))
    }
}

fn oid(value: String) -> Result<String, Error> {
    if valid_oid(&value) {
        Ok(value)
    } else {
        Err(Error::Upstream)
    }
}

fn valid_oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

fn object_route(scope: &Scope, kind: &str, id: &str) -> Result<String, Error> {
    if !valid_oid(id) {
        return Err(Error::Upstream);
    }
    Ok(format!("/repos/{}/git/{kind}/{id}", scope.full_name))
}
