use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::path::Path;
use std::time::Instant;

use ctxfab_inference::PROFILE;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Local, Reference, search};

const MODES: [&str; 3] = ["bm25", "semantic", "hybrid"];

#[derive(Deserialize)]
struct Judgment {
    id: String,
    repository: String,
    language: String,
    query: String,
    line: u32,
    anchor: String,
}

pub(super) fn rebuild(local: &Local) -> Result<Value, Box<dyn Error>> {
    let initial = local.read("/v1/retrieval")?;
    assert_eq!(initial["profile"], PROFILE);
    assert_eq!(initial["indexed"], 0);
    let total = initial["total"].as_u64().ok_or("passage count")?;
    assert!(total > 20);
    let started = Instant::now();
    let mut indexed = 0;
    for _ in 0..total.div_ceil(16) {
        let progress = local.write("POST", "/v1/retrieval/rebuild?limit=16", &Value::Null)?;
        let after = progress["indexed"].as_u64().ok_or("indexed count")?;
        assert!((indexed + 1..=indexed + 16).contains(&after));
        assert_eq!(progress["total"], total);
        indexed = after;
        if indexed == total {
            assert_eq!(progress["state"], "ready");
            eprintln!(
                "Code embedding index {}",
                json!({"profile": PROFILE, "passages": total, "buildMs": started.elapsed().as_millis()})
            );
            return Ok(progress);
        }
    }
    Err("embedding fixture did not finish its bounded rebuild".into())
}

pub(super) fn qualify(
    cli: &OsStr,
    data: &Path,
    corpus: &Path,
    references: &[Reference],
    required: bool,
) -> Result<(), Box<dyn Error>> {
    let judgments: Vec<Judgment> = serde_json::from_str(include_str!(
        "../../../../quality/m07/retrieval-judgments.json"
    ))?;
    let mut evaluated = HashSet::new();
    for reference in references {
        let warm = Instant::now();
        for mode in MODES {
            assert!(
                search(
                    cli,
                    data,
                    &reference.query,
                    reference.repository_id,
                    "unindexed",
                    mode
                )?
                .is_empty(),
                "{mode} must return no evidence for unindexed code"
            );
        }
        let initialized = search(
            cli,
            data,
            &reference.query,
            reference.repository_id,
            "fixture",
            "semantic",
        )?;
        assert_ne!(initialized, [] as [Value; 0]);
        eprintln!(
            "Code embedding warmup {} {}",
            reference.repository,
            warm.elapsed().as_millis()
        );
        let source = std::fs::read_to_string(corpus.join(&reference.file))?;
        let judged: Vec<_> = judgments
            .iter()
            .filter(|judgment| judgment.repository == reference.repository)
            .collect();
        for (language, count) in [("identifier", 1), ("en", 2), ("fr", 2)] {
            assert_eq!(
                judged
                    .iter()
                    .filter(|judgment| judgment.language == language)
                    .count(),
                count,
                "{} {language} coverage",
                reference.repository,
            );
        }
        for judgment in judged {
            assert!(
                evaluated.insert(judgment.id.as_str()),
                "duplicate judgment {}",
                judgment.id
            );
            qualify_query(cli, data, reference, &source, judgment, required)?;
        }
    }
    assert_eq!(
        evaluated.len(),
        judgments.len(),
        "every judgment must match a reference"
    );
    Ok(())
}

fn qualify_query(
    cli: &OsStr,
    data: &Path,
    reference: &Reference,
    source: &str,
    judgment: &Judgment,
    required: bool,
) -> Result<(), Box<dyn Error>> {
    assert!(
        source
            .lines()
            .nth(usize::try_from(judgment.line - 1)?)
            .is_some_and(|line| line.contains(&judgment.anchor))
    );
    for mode in MODES {
        let started = Instant::now();
        let hits = search(
            cli,
            data,
            &judgment.query,
            reference.repository_id,
            "fixture",
            mode,
        )?;
        let elapsed = started.elapsed().as_millis();
        let located = locate(&hits, reference, source, judgment)?;
        eprintln!(
            "Code retrieval judgment {}",
            json!({
                "id": judgment.id, "language": judgment.language, "repository": reference.repository,
                "mode": mode, "rank": located, "candidates": hits.len(), "elapsedMs": elapsed,
                "expectedLine": judgment.line,
                "match": located.map(|rank| &hits[rank - 1]["location"]),
            })
        );
        let gate = match (mode, judgment.language.as_str()) {
            (_, "identifier") | ("semantic", _) => Some(5),
            ("hybrid", _) => Some(10),
            _ => None,
        };
        if required && let Some(gate) = gate {
            assert!(
                located.is_some_and(|rank| rank <= gate),
                "{} missing from top {gate} under {mode}",
                judgment.id,
            );
        }
    }
    Ok(())
}

fn locate(
    hits: &[Value],
    reference: &Reference,
    source: &str,
    judgment: &Judgment,
) -> Result<Option<usize>, Box<dyn Error>> {
    let mut ranked = None;
    let mut seen = HashSet::new();
    assert!(hits.len() <= 20);
    for (index, hit) in hits.iter().enumerate() {
        let (locator, original) = reference.verify_location(hit, source)?;
        assert!(seen.insert(locator.start_byte), "duplicate passage");
        if (locator.start_line..=locator.end_line).contains(&judgment.line)
            && original.contains(&judgment.anchor)
        {
            ranked.get_or_insert(index + 1);
        }
    }
    Ok(ranked)
}
