use super::{GitHub, MetadataRequest};
use crate::application::telemetry::TraceContext;
use crate::domain::github::{
    Error, RepositoryName,
    branches::{MAX_PAGE, Page},
};

impl GitHub {
    /// # Errors
    /// Rejects unknown repositories, stale settings, missing credentials and provider failures.
    pub async fn branches(
        &self,
        id: i64,
        revision: u32,
        page: u16,
        trace: Option<TraceContext<'_>>,
    ) -> Result<Page, Error> {
        if id <= 0 || !(1..=MAX_PAGE).contains(&page) {
            return Err(Error::InvalidSelection);
        }
        let snapshot = self.store.snapshot()?;
        if snapshot.revision != revision {
            return Err(Error::Conflict);
        }
        let repository = snapshot
            .repositories
            .iter()
            .find(|repository| repository.repository.id == id)
            .ok_or(Error::Access)?;
        let name = RepositoryName::parse(&repository.repository.full_name)?;
        let (reference, secret) = self.secret(self.at_revision(revision)?)?;
        let guard = || self.at_revision(revision).map(|_| ());
        let source = MetadataRequest {
            secret: &secret,
            guard: &guard,
            trace,
        };
        let result = self.provider.branches(source, name, page).await;
        let page = self.observe(&reference, result)?;
        guard()?;
        Ok(page)
    }
}
