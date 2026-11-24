use super::SearchQuery;

const MAX_REFERENCES: usize = 4;

impl SearchQuery {
    pub(crate) fn markdown_readme_reference(&self) -> Option<&str> {
        let mut files = self.referenced_files();
        if let Some(file) = files.next() {
            return (files.next().is_none()
                && ["readme.md", "readme.markdown"]
                    .iter()
                    .any(|name| file.eq_ignore_ascii_case(name)))
            .then_some(file);
        }
        self.terms()
            .iter()
            .map(String::as_str)
            .find(|term| term.eq_ignore_ascii_case("readme"))
    }

    pub(crate) fn readme_authority_reference(&self) -> Option<&str> {
        let reference = self.markdown_readme_reference()?;
        let normalized = self.original().to_lowercase().replace('’', "'");
        let words: Vec<_> = normalized
            .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '.')
            .map(|word| word.trim_matches('.'))
            .filter(|word| !word.is_empty())
            .collect();
        if words.iter().any(|word| {
            matches!(
                *word,
                "compare"
                    | "compares"
                    | "versus"
                    | "vs"
                    | "differ"
                    | "differs"
                    | "comparer"
                    | "comparez"
                    | "diffère"
                    | "différence"
            )
        }) || words.windows(2).any(|pair| {
            matches!(
                pair,
                [
                    "and" | "et" | "or" | "ou",
                    "code" | "tests" | "history" | "changelog" | "actions" | "historique"
                ]
            )
        }) || words.windows(3).any(|triple| {
            matches!(
                triple,
                [
                    "and" | "et" | "or" | "ou",
                    "the" | "le" | "la" | "les",
                    "code" | "tests" | "history" | "changelog" | "actions" | "historique"
                ]
            )
        }) {
            return None;
        }
        let indices: Vec<_> = words
            .iter()
            .enumerate()
            .filter_map(|(index, word)| {
                matches!(*word, "readme" | "readme.md" | "readme.markdown").then_some(index)
            })
            .collect();
        let [index] = indices.as_slice() else {
            return None;
        };
        let before = &words[index.saturating_sub(6)..*index];
        let phrase = before
            .iter()
            .rposition(|word| matches!(*word, "selon" | "d'après"))
            .map(|start| &before[start + 1..])
            .or_else(|| {
                before
                    .windows(2)
                    .rposition(|pair| matches!(pair, ["according", "to"] | ["based", "on"]))
                    .map(|start| &before[start + 2..])
            });
        let explicit = phrase.is_some_and(|part| {
            !part
                .iter()
                .any(|word| matches!(*word, "and" | "et" | "or" | "ou"))
        });
        let direct = before.windows(2).any(|pair| pair == ["what", "does"])
            && words
                .get(index + 1)
                .is_some_and(|word| matches!(*word, "say" | "state" | "describe" | "document"));
        (explicit || direct || preposed_readme_scope(&normalized)).then_some(reference)
    }

    /// Qualified GitHub owner/repository syntax attached to the README designation.
    pub(crate) fn readme_authority_repository_hint(&self) -> Option<&str> {
        let words: Vec<_> = self
            .original()
            .split_whitespace()
            .map(|word| word.trim_matches(['`', '"', '\'', '.', ',', '?', '!', ':']))
            .collect();
        let index = words.iter().position(|word| {
            ["readme", "readme.md", "readme.markdown"]
                .iter()
                .any(|name| word.eq_ignore_ascii_case(name))
        })?;
        let slug = |word: &&str| {
            word.split_once('/').is_some_and(|(owner, repo)| {
                [owner, repo].into_iter().all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                })
            })
        };
        let before = index
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .copied()
            .filter(slug);
        let after = words
            .get(index + 1)
            .filter(|word| {
                ["of", "from", "de", "du"]
                    .iter()
                    .any(|connector| word.eq_ignore_ascii_case(connector))
            })
            .and_then(|_| words.get(index + 2))
            .copied()
            .filter(slug);
        before.or(after)
    }

    /// File-like literals in the question. The store resolves them within the search scope.
    pub(crate) fn referenced_files(&self) -> impl Iterator<Item = &str> {
        self.original()
            .split_whitespace()
            .map(|word| {
                word.trim_matches([
                    '`', '"', '\'', '(', ')', '[', ']', '{', '}', ',', ':', ';', '?', '!',
                ])
                .trim_end_matches('.')
            })
            .filter(|path| {
                let mut components = path.split('/');
                let name = components.next_back().unwrap_or_default();
                components.all(|part| !matches!(part, "" | "." | ".."))
                    && name.rsplit_once('.').is_some_and(|(stem, extension)| {
                        !stem.is_empty()
                            && extension
                                .as_bytes()
                                .first()
                                .is_some_and(u8::is_ascii_alphabetic)
                    })
                    && path.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
                    })
            })
            .take(MAX_REFERENCES)
    }

    /// Distinct compound identifiers, in question order.
    pub(crate) fn referenced_identifiers(&self) -> Vec<&str> {
        let mut identifiers = Vec::new();
        for word in self
            .original()
            .split(|character: char| character != '_' && !character.is_alphanumeric())
        {
            let mixed_case = word.bytes().skip(1).any(|byte| byte.is_ascii_uppercase())
                && word.bytes().any(|byte| byte.is_ascii_lowercase());
            if word.is_ascii()
                && (word.trim_matches('_').contains('_') || mixed_case)
                && !identifiers.contains(&word)
            {
                identifiers.push(word);
                if identifiers.len() == MAX_REFERENCES {
                    break;
                }
            }
        }
        identifiers
    }
}

// A leading prepositional clause can explicitly scope a single question.
fn preposed_readme_scope(question: &str) -> bool {
    let Some((source, body)) = question.split_once([',', ':']) else {
        return false;
    };
    // Require a single question after the clause, rather than a sentence
    // mentioning the README followed by a separate request.
    if !body.trim_end().ends_with('?') || body.contains(". ") || body.matches('?').count() != 1 {
        return false;
    }
    let terms: Vec<_> = source
        .split_whitespace()
        .map(|word| word.trim_matches(['`', '"', '\'']))
        .collect();
    let readme = |term| matches!(term, "readme" | "readme.md" | "readme.markdown");
    let project = |word: &str| {
        !word.is_empty()
            && !matches!(word, "and" | "or" | "et" | "ou")
            && word
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'/' | b'.'))
    };
    match terms.as_slice() {
        ["in", "the", qualifier @ .., file] => {
            readme(file) && qualifier.len() <= 3 && qualifier.iter().all(|word| project(word))
        }
        ["dans", "le", file] => readme(file),
        ["dans", "le", file, "de" | "du", name] => readme(file) && project(name),
        _ => false,
    }
}

#[cfg(test)]
mod preposed_readme_tests {
    use super::*;

    #[test]
    fn recognizes_a_preposed_readme_source_only_when_it_governs_the_question()
    -> Result<(), Box<dyn std::error::Error>> {
        for question in [
            "Dans le README de ProjectOne, comment active-t-on le mode local ?",
            "In the ProjectOne README, what enables local mode?",
            "In the README: what enables local mode?",
        ] {
            assert_eq!(
                SearchQuery::parse(question)?.readme_authority_reference(),
                Some("README"),
                "{question}",
            );
        }
        for question in [
            "Dans le README de ProjectOne, compare le mode local au code.",
            "In the ProjectOne README, how does the claim differ from the implementation?",
            "In the ProjectOne README, what does it say compared with config.xml?",
            "After seeing a link in the ProjectOne README, how does runtime logging work?",
            "I saw a reference in the README. What does the code actually do?",
            "In the ProjectOne README, I saw a link. What does the code actually do?",
        ] {
            assert_eq!(
                SearchQuery::parse(question)?.readme_authority_reference(),
                None,
                "{question}"
            );
        }
        Ok(())
    }
}
