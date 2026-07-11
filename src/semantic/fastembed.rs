use crate::error::{AppError, Result};
use crate::semantic::embed::SemanticEmbedder;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::path::PathBuf;

pub struct FastembedEmbedder {
    model: TextEmbedding,
}

impl FastembedEmbedder {
    pub fn new() -> Result<Self> {
        Self::new_with_download_progress(crate::semantic::cache::model_cache_dir(), true)
    }

    pub fn new_quiet() -> Result<Self> {
        Self::new_with_download_progress(crate::semantic::cache::model_cache_dir(), false)
    }

    pub fn cache_dir() -> PathBuf {
        crate::semantic::cache::model_cache_dir()
    }

    fn new_with_download_progress(
        cache_dir: PathBuf,
        show_download_progress: bool,
    ) -> Result<Self> {
        let init_options = InitOptions::new(EmbeddingModel::NomicEmbedTextV15)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(show_download_progress);
        let model = TextEmbedding::try_new(init_options)
            .map_err(|e| AppError::SemanticSearch(format!("fastembed init failed: {e}")))?;
        Ok(Self { model })
    }
    fn embed(&mut self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.model
            .embed(texts, None)
            .map_err(|e| AppError::SemanticSearch(format!("fastembed embed failed: {e}")))
    }
}

impl SemanticEmbedder for FastembedEmbedder {
    fn embed_passages(&mut self, passages: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(prefixed_passages(passages))
    }

    fn embed_query(&mut self, query: &str) -> Result<Option<Vec<f32>>> {
        let mut results = self.embed(vec![prefixed_query(query)])?;
        Ok(results.pop())
    }
}
fn prefixed_query(query: &str) -> String {
    format!("search_query: {query}")
}
fn prefixed_passages(passages: &[String]) -> Vec<String> {
    passages
        .iter()
        .map(|passage| format!("search_document: {passage}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{prefixed_passages, prefixed_query};

    #[test]
    fn prefixed_query_adds_search_query_prefix() {
        assert_eq!(prefixed_query("hello"), "search_query: hello");
        assert_eq!(prefixed_query(""), "search_query: ");
    }

    #[test]
    fn prefixed_passages_adds_search_document_prefix() {
        assert_eq!(
            prefixed_passages(&["a".into(), "b".into()]),
            vec![
                "search_document: a".to_string(),
                "search_document: b".to_string()
            ]
        );
        assert_eq!(prefixed_passages(&[]), Vec::<String>::new());
    }
}
