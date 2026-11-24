//! Client presentation of the user message selected by the completion budget.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct ModelContext {
    question: String,
    sources: Vec<Source>,
}

#[derive(Debug, Serialize)]
struct Source {
    slot: usize,
    kind: Kind,
    parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    change: Option<ChangeIdentity>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Current,
    History,
    Workflow,
}

#[derive(Debug, Serialize)]
struct Part {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<super::history::Side>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    source: Option<Location>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    path: String,
    revision: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Deserialize)]
struct Prompt {
    question: String,
    #[serde(flatten)]
    evidence: Evidence,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Evidence {
    Passages { passages: Vec<Passage> },
    Changes { changes: Vec<Change> },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Passage {
    id: usize,
    kind: Kind,
    text: String,
    source: Option<Location>,
    repository: Option<String>,
    branch: Option<String>,
    #[serde(flatten)]
    change: Option<ChangeIdentity>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ChangeIdentity {
    before_path: String,
    after_path: String,
    parent: String,
    child: String,
}

#[derive(Deserialize)]
struct Change {
    pair: usize,
    before: HistoryPart,
    after: HistoryPart,
}

#[derive(Deserialize)]
struct HistoryPart {
    path: String,
    revision: String,
    lines: Vec<Line>,
}

#[derive(Deserialize)]
struct Line {
    line: usize,
    text: String,
}

impl HistoryPart {
    fn present(self, side: super::history::Side) -> Part {
        let source = self
            .lines
            .first()
            .zip(self.lines.last())
            .map(|(first, last)| Location {
                path: self.path,
                revision: self.revision,
                start_line: first.line,
                end_line: last.line,
            });
        let texts: Vec<_> = self.lines.into_iter().map(|line| line.text).collect();
        Part {
            side: Some(side),
            text: texts.join("\n"),
            source,
        }
    }
}

impl ModelContext {
    pub(super) fn from_completion(prompt: Option<&str>) -> Option<Self> {
        let Prompt { question, evidence } = serde_json::from_str(prompt?).ok()?;
        let sources = match evidence {
            Evidence::Passages { passages } => passages
                .into_iter()
                .map(|passage| Source {
                    slot: passage.id,
                    kind: passage.kind,
                    parts: vec![Part {
                        text: passage.text,
                        side: None,
                        source: passage.source,
                    }],
                    repository: passage.repository,
                    branch: passage.branch,
                    change: passage.change,
                })
                .collect(),
            Evidence::Changes { changes } => changes
                .into_iter()
                .map(|change| Source {
                    slot: change.pair,
                    kind: Kind::History,
                    parts: vec![
                        change.before.present(super::history::Side::Before),
                        change.after.present(super::history::Side::After),
                    ],
                    repository: None,
                    branch: None,
                    change: None,
                })
                .collect(),
        };
        Some(Self { question, sources })
    }
}
