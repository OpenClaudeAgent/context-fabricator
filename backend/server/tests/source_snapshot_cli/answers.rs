use std::error::Error;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use ctxfab_inference::generation::GENERATION_PROFILE;
use serde_json::{Value, json};

use super::Reference;

pub(super) fn qualify(
    cli: &OsStr,
    data: &Path,
    fixture_dir: &Path,
    references: &[Reference],
    embeddings: bool,
    required: bool,
) -> Result<(), Box<dyn Error>> {
    let judgments = [
        (
            "psf/requests",
            "Dans get_adapter, quelle exception est levée si aucun préfixe ne correspond à l’URL ?",
            "InvalidSchema",
        ),
        (
            "BurntSushi/ripgrep",
            "Which punctuation does the choices_ascii_alphanumeric test allow in addition to letters and digits?",
            "is_ascii_alphanumeric",
        ),
    ];
    for (repository, question, anchor) in judgments {
        let reference = references
            .iter()
            .find(|reference| reference.repository == repository)
            .ok_or("missing reference")?;
        let response = ask(cli, data, question, reference.repository_id, "fixture")?;
        eprintln!("Scoped code answer {response}");
        assert_eq!(response["model"], GENERATION_PROFILE);
        assert_eq!(response["mode"], if embeddings { "hybrid" } else { "bm25" });
        let text = response["text"].as_str().unwrap_or_default();
        let correct = if repository == "psf/requests" {
            text.contains("InvalidSchema")
        } else {
            ['-', ':', '+']
                .into_iter()
                .all(|symbol| text.contains(symbol))
        };
        let supported = verify_quotes(&response, reference, fixture_dir, anchor)?;
        let accepted = response["outcome"] == "answered" && correct && supported;
        eprintln!(
            "Code chunking answer {}",
            json!({
                "repository": repository, "correctText": correct, "supported": supported,
                "accepted": accepted, "outcome": response["outcome"],
            })
        );
        assert!(
            !required || accepted,
            "expected a supported answer for {repository}"
        );
        let absent = ask(cli, data, question, reference.repository_id, "unindexed")?;
        assert_eq!(absent["outcome"], "insufficientEvidence");
        assert_eq!(absent["citations"], json!([]));
    }
    Ok(())
}

fn verify_quotes(
    response: &Value,
    reference: &Reference,
    fixture_dir: &Path,
    anchor: &str,
) -> Result<bool, Box<dyn Error>> {
    let citations = response["citations"].as_array().ok_or("code citations")?;
    let source = std::fs::read_to_string(fixture_dir.join(&reference.file))?;
    let mut supported = String::new();
    for citation in citations {
        let (_, prompt_excerpt) = reference.verify_location(&citation["evidence"], &source)?;
        let quote = citation["quote"].as_str().ok_or("quote text")?;
        assert!(prompt_excerpt.contains(quote));
        supported.push_str(quote);
        supported.push('\n');
    }
    Ok(if anchor == "is_ascii_alphanumeric" {
        supported.contains("[-+:")
            || ["c == '-'", "c == ':'", "c == '+'"]
                .into_iter()
                .all(|comparison| supported.contains(comparison))
    } else {
        supported.contains(anchor)
    })
}

fn ask(
    cli: &OsStr,
    data: &Path,
    question: &str,
    id: u64,
    branch: &str,
) -> Result<Value, Box<dyn Error>> {
    let output = Command::new(cli)
        .env("CTXFAB_DATA_DIRECTORY", data)
        .args([
            "ask",
            question,
            "--repository-id",
            &id.to_string(),
            "--branch",
            branch,
        ])
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
