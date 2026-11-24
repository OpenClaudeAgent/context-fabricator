use serde::Deserialize;

use crate::application::github::MetadataRequest;
use crate::domain::github::{
    Error, RepositoryName,
    branches::{MAX_PAGE, Page},
    valid_branch,
};
use crate::domain::sync::RequestKind;

use super::{GitHubRest, request::read_page};

const PAGE_SIZE: usize = 100;
const ROUTE: &str = "/repos/{owner}/{repo}/branches";

#[derive(Deserialize)]
struct Branch {
    name: String,
}

impl GitHubRest {
    pub(super) async fn list_branches(
        &self,
        source: MetadataRequest<'_>,
        name: RepositoryName<'_>,
        page: u16,
    ) -> Result<Page, Error> {
        let path = format!(
            "/repos/{}/{}/branches?per_page={PAGE_SIZE}&page={page}",
            name.owner, name.name
        );
        let kind = if page == 1 {
            RequestKind::Validation
        } else {
            RequestKind::Pagination
        };
        let (branches, next): (Vec<Branch>, bool) = self
            .get_metadata(source, kind, &path, ROUTE, read_page)
            .await?;
        if branches.len() > PAGE_SIZE || branches.iter().any(|branch| !valid_branch(&branch.name)) {
            return Err(Error::Upstream);
        }
        Ok(Page {
            branches: branches.into_iter().map(|branch| branch.name).collect(),
            next_page: (next && page < MAX_PAGE).then(|| page + 1),
        })
    }
}
