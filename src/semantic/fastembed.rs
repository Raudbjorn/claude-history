use crate::error::{AppError, Result};
use crate::semantic::embed::SemanticEmbedder;
use crate::semantic::types::DEFAULT_EMBEDDING_BATCH_SIZE;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::path::PathBuf;

const PYTHON_MODEL_NAME: &str = "BAAI/bge-small-en-v1.5";

pub struct FastembedEmbedder {
    model: Py<PyAny>,
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
        Python::attach(|py| {
            let fastembed = py
                .import("fastembed")
                .map_err(|error| python_error("import python-fastembed", error))?;
            let kwargs = PyDict::new(py);
            kwargs
                .set_item("model_name", PYTHON_MODEL_NAME)
                .map_err(|error| python_error("configure python-fastembed model", error))?;
            kwargs
                .set_item("cache_dir", cache_dir.to_string_lossy().as_ref())
                .map_err(|error| python_error("configure python-fastembed cache", error))?;

            if !show_download_progress {
                suppress_download_progress(py);
            }

            let model = fastembed
                .getattr("TextEmbedding")
                .and_then(|class| class.call((), Some(&kwargs)))
                .map_err(|error| python_error("initialize python-fastembed", error))?;
            Ok(Self {
                model: model.unbind(),
            })
        })
    }

    fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        Python::attach(|py| {
            let kwargs = PyDict::new(py);
            kwargs
                .set_item("batch_size", DEFAULT_EMBEDDING_BATCH_SIZE)
                .map_err(|error| python_error("configure python-fastembed batch", error))?;
            let output = self
                .model
                .bind(py)
                .call_method("embed", (texts,), Some(&kwargs))
                .map_err(|error| {
                    python_error("generate embeddings with python-fastembed", error)
                })?;
            collect_embeddings(&output)
        })
    }
}

impl SemanticEmbedder for FastembedEmbedder {
    fn embed_passages(&mut self, passages: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(prefixed_passages(passages))
    }

    fn embed_query(&mut self, query: &str) -> Result<Option<Vec<f32>>> {
        Ok(self.embed(vec![prefixed_query(query)])?.into_iter().next())
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

fn collect_embeddings(output: &Bound<'_, PyAny>) -> Result<Vec<Vec<f32>>> {
    let iterator = output
        .try_iter()
        .map_err(|error| python_error("iterate python-fastembed output", error))?;
    iterator
        .map(|item| {
            let embedding =
                item.map_err(|error| python_error("read python-fastembed output", error))?;
            embedding
                .call_method0("tolist")
                .and_then(|values| values.extract::<Vec<f32>>())
                .map_err(|error| python_error("convert python-fastembed output", error))
        })
        .collect()
}

fn suppress_download_progress(py: Python<'_>) {
    let _ = py
        .import("huggingface_hub.utils")
        .and_then(|module| module.call_method0("disable_progress_bars"));
}

fn python_error(context: &str, error: PyErr) -> AppError {
    AppError::ConfigError(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_query_for_fastembed() {
        assert_eq!(prefixed_query("rust cache"), "query: rust cache");
    }

    #[test]
    fn prefixes_passages_for_fastembed() {
        assert_eq!(
            prefixed_passages(&["one".to_string(), "two".to_string()]),
            vec!["passage: one".to_string(), "passage: two".to_string()]
        );
    }

    #[test]
    fn converts_python_numpy_embeddings_to_rust_vectors() {
        Python::attach(|py| {
            let numpy = py.import("numpy").expect("import numpy");
            let output = numpy
                .call_method1("array", (vec![vec![1.0_f32, 2.0], vec![3.0, 4.0]],))
                .expect("create numpy array");

            assert_eq!(
                collect_embeddings(&output).expect("convert embeddings"),
                vec![vec![1.0, 2.0], vec![3.0, 4.0]]
            );
        });
    }
}
