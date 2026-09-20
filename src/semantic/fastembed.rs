use crate::error::{AppError, Result};
use crate::semantic::embed::SemanticEmbedder;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use std::path::PathBuf;

#[cfg(feature = "release-dynamic-ort")]
use std::{path::Path, sync::OnceLock};

/// Passages per forward pass. Small on purpose: on CPU it embeds as fast as
/// 32 while the attention activations shrink resident memory from ~2.1 GB to
/// ~0.75 GB.
const EMBED_BATCH_SIZE: usize = 8;
/// Intra-op threads stop paying off around here on a small model.
const MAX_EMBED_THREADS: usize = 4;

pub struct FastembedEmbedder {
    model: TextEmbedding,
}

/// How many ONNX intra-op threads one embedding call may use. Batch paths
/// (the CLI, the agent) have nothing else to run; the TUI's worker leaves a
/// core for the event loop so typing stays responsive while a prewarm runs
/// (the reason 6ce770f pinned everything to one thread). Thread count only
/// changes the order of floating-point reductions, so embeddings stay
/// equivalent and the embedding cache remains valid.
///
/// Measured on a 6-core/12-thread laptop: 4 threads were 1.75x faster than
/// one, 6 slightly slower than 4, and 12 slower still at 2.6x the CPU;
/// several one-thread sessions run data-parallel matched 4 intra-op threads
/// at 3x the memory. Hence one session with at most [`MAX_EMBED_THREADS`]
/// threads; a desktop with more physical cores may scale differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedThreads {
    AllCores,
    LeaveOneForUi,
}

impl EmbedThreads {
    /// Logical cores are assumed to be hyperthreaded pairs, which is
    /// conservative on machines without SMT.
    pub fn count(self) -> usize {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        let physical = (logical / 2).max(1);
        match self {
            Self::AllCores => physical.min(MAX_EMBED_THREADS),
            Self::LeaveOneForUi => physical.saturating_sub(1).clamp(1, MAX_EMBED_THREADS),
        }
    }
}

impl FastembedEmbedder {
    pub fn new(threads: EmbedThreads) -> Result<Self> {
        Self::new_with_download_progress(crate::semantic::cache::model_cache_dir(), true, threads)
    }

    pub fn new_quiet(threads: EmbedThreads) -> Result<Self> {
        Self::new_with_download_progress(crate::semantic::cache::model_cache_dir(), false, threads)
    }

    pub fn cache_dir() -> PathBuf {
        crate::semantic::cache::model_cache_dir()
    }

    fn new_with_download_progress(
        cache_dir: PathBuf,
        show_download_progress: bool,
        threads: EmbedThreads,
    ) -> Result<Self> {
        init_onnx_runtime()?;
        let model = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(show_download_progress)
                .with_intra_threads(threads.count()),
        )
        .map_err(to_config_error)?;
        Ok(Self { model })
    }
}

impl SemanticEmbedder for FastembedEmbedder {
    fn embed_passages(&mut self, passages: &[String]) -> Result<Vec<Vec<f32>>> {
        self.model
            .embed(prefixed_passages(passages), Some(EMBED_BATCH_SIZE))
            .map_err(to_config_error)
    }

    fn embed_query(&mut self, query: &str) -> Result<Option<Vec<f32>>> {
        let embeddings = self
            .model
            .embed(vec![prefixed_query(query)], Some(EMBED_BATCH_SIZE))
            .map_err(to_config_error)?;
        Ok(embeddings.first().cloned())
    }
}

pub fn prefixed_query(query: &str) -> String {
    format!("Represent this sentence for searching relevant passages: {query}")
}

pub fn prefixed_passages(passages: &[String]) -> Vec<String> {
    passages.to_vec()
}

#[cfg(feature = "release-dynamic-ort")]
fn init_onnx_runtime() -> Result<()> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(initialize_bundled_onnx_runtime) {
        Ok(()) => Ok(()),
        Err(message) => Err(AppError::ConfigError(message.clone())),
    }
}

#[cfg(not(feature = "release-dynamic-ort"))]
fn init_onnx_runtime() -> Result<()> {
    Ok(())
}

#[cfg(feature = "release-dynamic-ort")]
fn initialize_bundled_onnx_runtime() -> std::result::Result<(), String> {
    let path = bundled_onnx_runtime_path().ok_or_else(|| {
        format!(
            "Bundled ONNX Runtime library was not found beside the executable; expected a compatible {}",
            runtime_library_name()
        )
    })?;
    let builder = ort::init_from(&path).map_err(|error| {
        format!(
            "Failed to load bundled ONNX Runtime from {}: {error}",
            path.display()
        )
    })?;
    if !builder.commit() {
        return Err(format!(
            "Failed to initialize bundled ONNX Runtime from {}: an ONNX Runtime environment is already configured",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(feature = "release-dynamic-ort")]
fn bundled_onnx_runtime_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_onnx_runtime_path_in_dir(exe.parent()?)
}

#[cfg(feature = "release-dynamic-ort")]
fn bundled_onnx_runtime_path_in_dir(dir: &Path) -> Option<PathBuf> {
    onnx_runtime_candidates(dir)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

#[cfg(feature = "release-dynamic-ort")]
fn onnx_runtime_candidates(dir: &Path) -> Vec<PathBuf> {
    let name = runtime_library_name();
    let lib_dir = dir.join("lib");
    let mut candidates = vec![dir.join(name), lib_dir.join(name)];

    for library_dir in [dir, lib_dir.as_path()] {
        let Ok(entries) = std::fs::read_dir(library_dir) else {
            continue;
        };
        let mut versioned = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| is_versioned_runtime_name(path, name))
            .collect::<Vec<_>>();
        versioned.sort();
        candidates.extend(versioned);
    }
    candidates
}

#[cfg(feature = "release-dynamic-ort")]
fn runtime_library_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "libonnxruntime.dylib",
        "windows" => "onnxruntime.dll",
        _ => "libonnxruntime.so",
    }
}

#[cfg(feature = "release-dynamic-ort")]
fn is_versioned_runtime_name(path: &Path, unversioned_name: &str) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match unversioned_name {
        "libonnxruntime.dylib" => {
            file_name.starts_with("libonnxruntime.")
                && file_name.ends_with(".dylib")
                && file_name != unversioned_name
        }
        "onnxruntime.dll" => false,
        _ => file_name.starts_with("libonnxruntime.so.") && file_name != unversioned_name,
    }
}

fn to_config_error(err: impl std::fmt::Display) -> AppError {
    AppError::ConfigError(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_query_for_fastembed() {
        assert_eq!(
            prefixed_query("rust cache"),
            "Represent this sentence for searching relevant passages: rust cache"
        );
    }

    #[test]
    fn leaves_passages_unprefixed_for_fastembed() {
        assert_eq!(
            prefixed_passages(&["one".to_string(), "two".to_string()]),
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[cfg(not(feature = "release-dynamic-ort"))]
    #[test]
    fn non_release_runtime_initialization_is_a_noop() {
        assert!(init_onnx_runtime().is_ok());
    }

    #[cfg(feature = "release-dynamic-ort")]
    #[test]
    fn runtime_path_uses_the_host_library_name() {
        let temp = tempfile::tempdir().unwrap();
        let lib_dir = temp.path().join("lib");
        std::fs::create_dir(&lib_dir).unwrap();
        std::fs::write(lib_dir.join("libonnxruntime.dylib"), b"wrong host").unwrap();
        std::fs::write(lib_dir.join(runtime_library_name()), b"runtime").unwrap();

        assert_eq!(
            bundled_onnx_runtime_path_in_dir(temp.path()),
            Some(lib_dir.join(runtime_library_name()))
        );
    }

    #[cfg(feature = "release-dynamic-ort")]
    #[test]
    fn runtime_path_falls_back_to_a_versioned_library() {
        let temp = tempfile::tempdir().unwrap();
        let lib_dir = temp.path().join("lib");
        std::fs::create_dir(&lib_dir).unwrap();
        let versioned_name = if runtime_library_name() == "libonnxruntime.dylib" {
            "libonnxruntime.1.24.2.dylib"
        } else {
            "libonnxruntime.so.1.24.2"
        };
        let versioned = lib_dir.join(versioned_name);
        std::fs::write(&versioned, b"runtime").unwrap();

        assert_eq!(
            bundled_onnx_runtime_path_in_dir(temp.path()),
            Some(versioned)
        );
    }
}
