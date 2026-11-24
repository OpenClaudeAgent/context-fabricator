//! Synthesis from a declaration and its bounded executable use.

use ctxfab_inference::generation::GenerationAttempt;

use super::{
    AnswerError, Answers, Citation, Draft, Event, Excerpt, GenerationEvidence, ModelContext,
    Observer, Request, SearchQuery, Stage, TraceLink, VerifiedQuote, json, linked, mixed,
    observe_generation, quantities,
};

pub(super) struct Proof {
    pub(super) draft: Draft,
    pub(super) citations: Vec<Citation>,
    pub(super) repository_id: i64,
    pub(super) model_context: Option<ModelContext>,
}

pub(super) enum Outcome {
    Skipped,
    Unsupported,
    Invalidated,
    Verified(Proof),
}

impl Answers {
    pub(super) fn prove_linked(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        head: &Excerpt,
        language: &str,
        observer: Option<&dyn Observer>,
        trace: Option<TraceLink>,
    ) -> Result<Outcome, AnswerError> {
        let Some(usage) = linked::find(self, head)? else {
            return Ok(Outcome::Skipped);
        };
        let Some(prompt) = prompt(query, language, head, &usage) else {
            return Ok(Outcome::Skipped);
        };
        let GenerationAttempt::Completed(result) = observe_generation(
            observer,
            trace,
            GenerationEvidence::LinkedPassages,
            |record| request.generate_linked(record, || prompt.clone()),
        )?
        else {
            return Ok(Outcome::Unsupported);
        };
        let draft = result.draft;
        let Some(quotes) = verify_shapes(head, &usage, &draft) else {
            return Ok(Outcome::Unsupported);
        };
        let verification = observer.map(|observer| observer.start(Stage::AnswerValidation, trace));
        let citations = self.reopen_citations(
            [head, &usage.excerpt].into_iter(),
            Vec::from(quotes),
            Some(&usage.scope),
        );
        if let Some(mut verification) = verification {
            if let Ok(citations) = &citations {
                verification.event(if citations.is_some() {
                    Event::LinkedEvidenceVerified
                } else {
                    Event::CitationsRejected
                });
            }
            verification.finish(matches!(citations, Ok(Some(_))));
        }
        Ok(match citations? {
            Some(citations) => Outcome::Verified(Proof {
                draft,
                citations,
                repository_id: usage.scope.repository_id,
                model_context: ModelContext::from_completion(result.completion_prompt.as_deref()),
            }),
            None => Outcome::Invalidated,
        })
    }
}

fn prompt(
    query: &SearchQuery,
    language: &str,
    declaration: &Excerpt,
    usage: &linked::LinkedUsage,
) -> Option<String> {
    let mut head = mixed::code_passage(declaration);
    head["id"] = json!(1);
    let location = usage.excerpt.evidence.location.as_ref()?;
    let source = &usage.excerpt.text;
    let shown = source
        .get(usage.visible.clone())?
        .trim_end_matches(['\r', '\n']);
    let before = source.get(..usage.visible.start)?.matches('\n').count();
    let first = location
        .start_line
        .checked_add(u32::try_from(before).ok()?)?;
    let last = first.checked_add(u32::try_from(shown.matches('\n').count()).ok()?)?;
    let use_site = json!({"id":2,"kind":"current","text":shown,"source":{
        "path":location.path,"revision":usage.excerpt.evidence.revision,"startLine":first,"endLine":last}});
    Some(
        json!({"question":query.original(),"answerLanguage":language,"passages":[head,use_site]})
            .to_string()
            .replace('<', "\\u003c")
            .replace('>', "\\u003e"),
    )
}

fn verify_shapes<'a>(
    head: &'a Excerpt,
    usage: &'a linked::LinkedUsage,
    draft: &Draft,
) -> Option<[VerifiedQuote<'a>; 2]> {
    let [first, second] = draft.citations.as_slice() else {
        return None;
    };
    let (declaration, reference) = match (first.passage, second.passage) {
        (1, 2) => (first, second),
        (2, 1) => (second, first),
        _ => return None,
    };
    let declaration_line = VerifiedQuote::exact_span(head, usage.declaration.clone())?;
    let reference_line = VerifiedQuote::exact_span(&usage.excerpt, usage.usage.clone())?;
    if declaration.quote.trim() != declaration_line.text
        || reference.quote.trim() != reference_line.text
        || usage.usage.start < usage.visible.start
        || usage.usage.end > usage.visible.end
        || quantities::integer_constant(declaration_line.text)?.0 != usage.symbol
        || !quantities::matches_declaration(&draft.answer, declaration_line.text)
    {
        return None;
    }
    Some([declaration_line, reference_line])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{CodeScope, SearchHit};
    use ctxfab_inference::generation::Reference;

    fn excerpt(text: &str) -> Excerpt {
        Excerpt {
            evidence: SearchHit {
                source: "fixture".into(),
                revision: "pin".into(),
                kind: "current".into(),
                tracked_ref: Some("refs/heads/main".into()),
                title: "fixture".into(),
                access: "available",
                location: None,
            },
            text: text.into(),
            repository_id: Some(7),
        }
    }

    #[test]
    fn requires_both_exact_visible_lines_and_the_declared_value() {
        let declaration = "pub const WAIT_MS: u64 = 1_000;";
        let statement = "    let deadline = now + WAIT_MS;";
        let head = excerpt(declaration);
        let mut usage = linked::LinkedUsage {
            excerpt: excerpt(statement),
            visible: 0..statement.len(),
            usage: 0..statement.len(),
            declaration: 0..declaration.len(),
            symbol: "WAIT_MS".into(),
            scope: CodeScope {
                repository_id: 7,
                branch: "main".into(),
            },
        };
        let mut draft = Draft {
            answer: "1 seconde".into(),
            citations: vec![
                Reference {
                    passage: 1,
                    quote: declaration.into(),
                },
                Reference {
                    passage: 2,
                    quote: statement.trim().into(),
                },
            ],
        };
        assert!(verify_shapes(&head, &usage, &draft).is_some());
        draft.citations.swap(0, 1);
        assert!(verify_shapes(&head, &usage, &draft).is_some());
        usage.visible.start = 4;
        assert!(verify_shapes(&head, &usage, &draft).is_none());
        usage.visible.start = 0;
        draft.answer = "2 secondes".into();
        assert!(verify_shapes(&head, &usage, &draft).is_none());
        draft.answer = "1000 ms".into();
        draft.citations[0].quote = "now + WAIT_MS".into();
        assert!(verify_shapes(&head, &usage, &draft).is_none());
        draft.citations.pop();
        assert!(verify_shapes(&head, &usage, &draft).is_none());
    }
}
