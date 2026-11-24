//! Canonical object IDs and pinned public source locators.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use super::RepositoryName;
use crate::domain::corpus::snapshot::Location;

const PATH_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'.')
    .remove(b'-')
    .remove(b'_')
    .remove(b'~');

#[must_use]
pub fn valid_object_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[must_use]
pub fn pinned_file_url(repository: &str, commit: &str, path: &str) -> String {
    let encoded_path = path
        .split('/')
        .map(|part| utf8_percent_encode(part, PATH_COMPONENT).to_string())
        .collect::<Vec<_>>()
        .join("/");
    format!("https://github.com/{repository}/blob/{commit}/{encoded_path}")
}

pub(crate) fn pinned_line_url(
    location: &Location,
    revision: &str,
    first: u32,
    last: u32,
) -> Option<String> {
    if !valid_object_id(revision)
        || first == 0
        || first < location.start_line
        || last < first
        || last > location.end_line
    {
        return None;
    }
    let mut parts = location
        .file_url
        .strip_prefix("https://github.com/")?
        .split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    let repository = format!("{owner}/{name}");
    let validated = RepositoryName::parse(&repository).ok()?;
    let url = pinned_file_url(&repository, revision, &location.path);
    if validated.owner != owner || validated.name != name || location.file_url != url {
        return None;
    }
    Some(if first == last {
        format!("{url}#L{first}")
    } else {
        format!("{url}#L{first}-L{last}")
    })
}
