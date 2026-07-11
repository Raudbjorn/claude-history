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
        _show_download_progress: bool,
    ) -> Result<Self> {
        let init_options =
            InitOptions::new(EmbeddingModel::BGEBaseENV15).with_cache_dir(cache_dir);
        let model = TextEmbedding::try_new(init_options)
            .map_err(|e| AppError::ConfigError(format!("fastembed init failed: {e}")))?;
        Ok(Self { model })
    }
    fn embed(&mut self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.model
            .embed(texts, None)
            .map_err(|e| AppError::ConfigError(format!("fastembed embed failed: {e}")))
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

pub fn prefixed_query(query: &str) -> String {
    format!("query: {query}")
}

pub fn prefixed_passages(passages: &[String]) -> Vec<String> {
    passages
        .iter()
        .map(|passage| format!("passage: {passage}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
    use std::path::PathBuf;

    fn make_model() -> TextEmbedding {
        let cache_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-cache");
        std::fs::create_dir_all(&cache_dir).ok();
        let init_options =
            InitOptions::new(EmbeddingModel::BGEBaseENV15).with_cache_dir(cache_dir);
        TextEmbedding::try_new(init_options).expect("init fastembed")
    }

    #[test]
    fn prefixes_affect_embeddings() {
        let mut model = make_model();
        let raw = model.embed(vec!["test query".to_string()], None).unwrap();
        let prefixed = model
            .embed(vec!["query: test query".to_string()], None)
            .unwrap();

        // cosine similarity should be well below 1.0 if prefixes change the embedding
        let dot: f32 = raw[0]
            .iter()
            .zip(prefixed[0].iter())
            .map(|(a, b)| a * b)
            .sum();
        let norm_raw: f32 = raw[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_prefixed: f32 = prefixed[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        let cos_sim = dot / (norm_raw * norm_prefixed);

        // With BGE, "query: " prefix should significantly changes the embedding substantially
        assert!(
            cos_sim < 0.99,
            "prefix must affect embedding (cos_sim={cos_sim})"
        );
    }

    #[test]
    fn passage_prefix_affects_embeddings() {
        let mut model = make_model();
        let raw = model.embed(vec!["test passage".to_string()], None).unwrap();
        let prefixed = model
            .embed(vec!["passage: test passage".to_string()], None)
            .unwrap();

        let dot: f32 = raw[0]
            .iter()
            .zip(prefixed[0].iter())
            .map(|(a, b)| a * b)
            .sum();
        let norm_raw: f32 = raw[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_prefixed: f32 = prefixed[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        let cos_sim = dot / (norm_raw * norm_prefixed);

        assert!(
            cos_sim < 0.99,
            "passage prefix must affect embedding (cos_sim={cos_sim})"
        );
    }
}
