use crate::{EmbeddingError, Input};

pub const PROFILE: &str = "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1";
pub const MAX_PASSAGE_BYTES: usize = 3072;

pub(super) fn encode(input: Input<'_>) -> Result<String, EmbeddingError> {
    match input {
        Input::Passage { title, content } => {
            let bytes_with_separator = title.len().saturating_add(content.len()).saturating_add(1);
            if bytes_with_separator > MAX_PASSAGE_BYTES {
                return Err(EmbeddingError::PassageTooLarge);
            }
            Ok(format!("{title}\n{content}"))
        }
        Input::Query(query) => Ok(format!(
            "Instruct: Given an engineering question, retrieve source passages that answer it.\nQuery: {query}"
        )),
    }
}
