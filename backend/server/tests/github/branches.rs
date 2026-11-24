use super::memory::Credentials;
use super::{
    Arc, BUCKET, Error, FIRST, Origin, Path, SqliteStore, TrafficLedger, Value, authorize, call,
    selection,
};
use serde_json::json;

#[tokio::test]
async fn lists_configured_branches_one_counted_page_at_a_time() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let saved = call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    let before = store.traffic(BUCKET)?.attempts;

    let first = call(
        &app,
        "GET",
        "/v1/github/repositories/41/branches?revision=2",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(
        first,
        json!({"branches": ["main", "feature/search"], "nextPage": 2})
    );
    assert_eq!(store.traffic(BUCKET)?.attempts, before + 1);
    let second = call(
        &app,
        "GET",
        "/v1/github/repositories/41/branches?revision=2&page=2",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(second, json!({"branches": ["release/v1"]}));

    for (path, status, code) in [
        (
            "/v1/github/repositories/42/branches?revision=2",
            403,
            "github_access_not_verified",
        ),
        (
            "/v1/github/repositories/41/branches?revision=1",
            409,
            "github_revision_conflict",
        ),
        (
            "/v1/github/repositories/41/branches?revision=2&page=0",
            400,
            "github_invalid_selection",
        ),
    ] {
        let rejected = call(&app, "GET", path, Value::Null, status).await?;
        assert_eq!(rejected["code"], code);
    }
    assert_eq!(store.traffic(BUCKET)?.attempts, before + 2);
    let branch_requests: Vec<_> = origin
        .hits()?
        .lines()
        .filter(|line| line.contains("/branches?per_page=100"))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        branch_requests,
        [
            "GET /repos/fixture/alpha/branches?per_page=100&page=1",
            "GET /repos/fixture/alpha/branches?per_page=100&page=2"
        ]
    );
    let after = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(after["revision"], saved["revision"]);
    assert_eq!(after["repositories"], saved["repositories"]);
    Ok(())
}
