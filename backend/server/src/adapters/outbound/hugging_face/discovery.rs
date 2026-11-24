use std::time::Duration;

use tokio::time::timeout;

use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, StatusCode, header};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, de::DeserializeOwned};

use super::HuggingFace;
use crate::application::model_management::{Discovery, DiscoveryResult};
use crate::domain::model::{
    Artifact, ArtifactKind, ModelSource,
    discovery::{
        DiscoveryError, Repository, Variant, hex, managed_embedding_profile, managed_profile,
        repository_valid, variant_quantization,
    },
};
use crate::domain::sync::{Reply, RequestKind};

const EXPANSIONS: &str = "expand%5B%5D=sha&expand%5B%5D=private&expand%5B%5D=gated&expand%5B%5D=gguf&expand%5B%5D=baseModels&expand%5B%5D=pipeline_tag";
const GENERATION_BASE: &str = "Qwen/Qwen3-4B-Instruct-2507";
const EMBEDDING_BASE: &str = "Qwen/Qwen3-Embedding-0.6B";
const EMBEDDING_METADATA: &str = "&expand%5B%5D=cardData&expand%5B%5D=tags";
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODEL_BYTES: u64 = 5_000_000_000;

#[derive(Deserialize)]
struct HubModel {
    id: String,
    sha: Option<String>,
    private: Option<bool>,
    gated: Option<serde_json::Value>,
    pipeline_tag: Option<String>,
    gguf: Option<Format>,
    #[serde(rename = "cardData")]
    card_data: Option<Card>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "baseModels")]
    base_models: Option<BaseModels>,
}

#[derive(Deserialize)]
struct Format {
    architecture: Option<String>,
    pooling_type: Option<u8>,
}

#[derive(Deserialize)]
struct Card {
    license: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct BaseModels {
    relation: String,
    models: Vec<ModelId>,
}

#[derive(Deserialize)]
struct ModelId {
    id: String,
}

impl HubModel {
    fn compatible(self, kind: ArtifactKind) -> Option<Repository> {
        let revision = self.sha?;
        let bases = self.base_models?;
        let format = self.gguf?;
        if !repository_valid(&self.id)
            || !hex(&revision, 40)
            || self.private != Some(false)
            || self.gated != Some(serde_json::Value::Bool(false))
            || format.architecture.as_deref() != Some("qwen3")
            || bases.relation != "quantized"
            || bases.models.len() != 1
        {
            return None;
        }
        let base = &bases.models[0].id;
        let license = match kind {
            ArtifactKind::Generation
                if base == GENERATION_BASE
                    && self.pipeline_tag.as_deref() == Some("text-generation") =>
            {
                None
            }
            ArtifactKind::Embedding
                if base == EMBEDDING_BASE
                    && matches!(self.pipeline_tag.as_deref(), Some("feature-extraction" | "sentence-similarity"))
                    // llama.cpp pooling type 3 uses the last token, as required by Qwen3 Embedding.
                    && format.pooling_type.is_none_or(|pooling| pooling == 3) =>
            {
                let card = self.card_data.and_then(|card| card.license);
                let tagged = self
                    .tags
                    .iter()
                    .filter_map(|tag| tag.strip_prefix("license:"))
                    .collect::<Vec<_>>();
                let card_approved =
                    card.as_ref().and_then(serde_json::Value::as_str) == Some("apache-2.0");
                if (card.is_some() && !card_approved)
                    || tagged.iter().any(|license| *license != "apache-2.0")
                    || (!card_approved && tagged.is_empty())
                {
                    return None;
                }
                Some("apache-2.0".into())
            }
            _ => return None,
        };
        Some(Repository {
            url: format!("https://huggingface.co/{}/tree/{revision}", self.id),
            repository: self.id,
            revision,
            kind: Some(kind),
            license,
        })
    }
}

fn expansions(kind: ArtifactKind) -> String {
    match kind {
        ArtifactKind::Generation => EXPANSIONS.into(),
        ArtifactKind::Embedding => format!("{EXPANSIONS}{EMBEDDING_METADATA}"),
    }
}

#[derive(Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    size: Option<u64>,
    lfs: Option<Lfs>,
}

#[derive(Deserialize)]
struct Lfs {
    oid: String,
    size: u64,
}

impl TreeEntry {
    fn downloadable(self) -> Option<Variant> {
        let lfs = self.lfs?;
        let quantization = variant_quantization(&self.path)?;
        if self.kind != "file"
            || self.size != Some(lfs.size)
            || !(1..=MAX_MODEL_BYTES).contains(&lfs.size)
            || !hex(&lfs.oid, 64)
        {
            return None;
        }
        Some(Variant {
            file: self.path,
            quantization,
            bytes: lfs.size,
            sha256: Some(lfs.oid),
        })
    }
}

impl HuggingFace {
    async fn search_hub(
        &self,
        query: &str,
        kind: ArtifactKind,
    ) -> Result<Vec<Repository>, DiscoveryError> {
        let query = query.trim();
        let exact = query
            .strip_prefix("https://huggingface.co/")
            .unwrap_or(query);
        if query.chars().any(char::is_control) {
            return Err(DiscoveryError::Invalid);
        }
        if repository_valid(exact) {
            let model: HubModel = self
                .metadata(&format!("/api/models/{exact}?{}", expansions(kind)))
                .await?;
            return Ok(model
                .compatible(kind)
                .filter(|model| model.repository == exact)
                .into_iter()
                .collect());
        }
        if query.chars().count() > 80 || query.contains("://") {
            return Err(DiscoveryError::Invalid);
        }
        let base = match kind {
            ArtifactKind::Generation => GENERATION_BASE,
            ArtifactKind::Embedding => EMBEDDING_BASE,
        };
        let term = if query.is_empty() {
            base.split_once('/').map_or(base, |(_, model)| model)
        } else {
            query
        };
        let term = utf8_percent_encode(term, NON_ALPHANUMERIC);
        let base_filter = utf8_percent_encode(base, NON_ALPHANUMERIC);
        let path = format!(
            "/api/models?filter=gguf&filter=base_model%3Aquantized%3A{base_filter}&search={term}&limit=20&sort=downloads&{}",
            expansions(kind)
        );
        let results: Vec<HubModel> = self.metadata(&path).await?;
        Ok(results
            .into_iter()
            .take(20)
            .filter_map(|model| model.compatible(kind))
            .collect())
    }

    async fn repo_files(
        &self,
        repository: &str,
        revision: &str,
        kind: ArtifactKind,
    ) -> Result<(Repository, Vec<Variant>), DiscoveryError> {
        if !repository_valid(repository) || !hex(revision, 40) {
            return Err(DiscoveryError::Invalid);
        }
        let model: HubModel = self
            .metadata(&format!(
                "/api/models/{repository}/revision/{revision}?{}",
                expansions(kind)
            ))
            .await?;
        let approved = model
            .compatible(kind)
            .filter(|model| model.repository == repository && model.revision == revision)
            .ok_or(DiscoveryError::Unsupported)?;
        let entries: Vec<TreeEntry> = self
            .metadata(&format!(
                "/api/models/{repository}/tree/{revision}?recursive=false"
            ))
            .await?;
        let mut files: Vec<_> = entries
            .into_iter()
            .filter_map(TreeEntry::downloadable)
            .collect();
        files.sort_unstable_by(|a, b| a.bytes.cmp(&b.bytes).then(a.file.cmp(&b.file)));
        files.truncate(12);
        Ok((approved, files))
    }

    async fn metadata<T: DeserializeOwned>(&self, path: &str) -> Result<T, DiscoveryError> {
        let request = Request::get(format!("{}{path}", self.scope.origin()))
            .header(header::ACCEPT, "application/json")
            .header(header::USER_AGENT, "ContextFabricator")
            .body(Full::default())
            .map_err(|_| DiscoveryError::Invalid)?;
        // Discovery has a bounded wait even while a transfer holds the shared permit.
        let reply = timeout(
            Duration::from_secs(15),
            self.transport.send(
                RequestKind::ModelCatalog,
                request,
                Duration::from_secs(12),
                async |response| match response.status() {
                    StatusCode::OK => match Limited::new(response.body_mut(), MAX_JSON_BYTES)
                        .collect()
                        .await
                    {
                        Ok(body) => serde_json::from_slice::<T>(&body.to_bytes())
                            .map_or(Reply::Temporary, Reply::Success),
                        Err(_) => Reply::Temporary,
                    },
                    StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => {
                        Reply::Permanent
                    }
                    _ => Reply::Temporary,
                },
            ),
        )
        .await
        .map_err(|_| DiscoveryError::Unavailable)?
        .map_err(|_| DiscoveryError::Unavailable)?;
        match reply {
            Reply::Success(value) => Ok(value),
            Reply::Throttled(..) => Err(DiscoveryError::RateLimited),
            Reply::Permanent => Err(DiscoveryError::Unsupported),
            _ => Err(DiscoveryError::Unavailable),
        }
    }
}

impl Discovery for HuggingFace {
    fn search<'a>(
        &'a self,
        query: &'a str,
        kind: ArtifactKind,
    ) -> DiscoveryResult<'a, Vec<Repository>> {
        Box::pin(self.search_hub(query, kind))
    }

    fn variants<'a>(
        &'a self,
        repository: &'a str,
        revision: &'a str,
        kind: ArtifactKind,
    ) -> DiscoveryResult<'a, Vec<Variant>> {
        Box::pin(async move {
            let (_, files) = self.repo_files(repository, revision, kind).await?;
            Ok(files)
        })
    }

    fn resolve<'a>(
        &'a self,
        repository: &'a str,
        revision: &'a str,
        file: &'a str,
        kind: ArtifactKind,
    ) -> DiscoveryResult<'a, Artifact> {
        Box::pin(async move {
            if variant_quantization(file).is_none() {
                return Err(DiscoveryError::Unsupported);
            }
            let (source, files) = self.repo_files(repository, revision, kind).await?;
            let file = files
                .into_iter()
                .find(|entry| entry.file == file)
                .ok_or(DiscoveryError::Unsupported)?;
            let pin = file.sha256.ok_or(DiscoveryError::Unsupported)?;
            let (publisher, _) = repository.split_once('/').ok_or(DiscoveryError::Invalid)?;
            let (profile, label) = match kind {
                ArtifactKind::Generation => (
                    managed_profile(repository, revision, &file.file),
                    "Qwen3 4B",
                ),
                ArtifactKind::Embedding => (
                    managed_embedding_profile(repository, revision, &file.file, &pin),
                    "Qwen3 Embedding 0.6B",
                ),
            };
            Ok(Artifact {
                label: Some(format!("{label} · {} · {publisher}", file.quantization).into()),
                kind: Some(kind),
                source: Some(ModelSource {
                    license: source.license.map(Into::into),
                    label: format!("Hugging Face · {publisher}").into(),
                    repository: source.repository.into(),
                    revision: source.revision.into(),
                    url: source.url.into(),
                }),
                file: format!("{profile}.gguf").into(),
                profile: profile.into(),
                bytes: file.bytes,
                sha256: pin.into(),
                remote_file: Some(file.file.into()),
            })
        })
    }
}
