//! "This project": the directory a command runs in, and the one rule for
//! whether a conversation belongs to it. `--local`, the TUI's `Tab` scope,
//! `--debug-search --local`, semantic `--local` and agent key discovery all
//! ask here, so Claude, Pi and OMP sessions are scoped the same way
//! everywhere.

use super::{Conversation, Source, convert_path_to_project_dir_name, is_same_project};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Workspace {
    /// Claude's encoding of the directory (`-home-me-repo`).
    dir_name: String,
    /// The directory with symlinks resolved, for Pi/OMP session headers.
    canonical: PathBuf,
}

impl Workspace {
    /// The process's current directory.
    pub fn current() -> std::io::Result<Self> {
        Ok(Self::at(&std::env::current_dir()?))
    }

    pub fn at(dir: &Path) -> Self {
        Self {
            dir_name: convert_path_to_project_dir_name(dir),
            canonical: dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()),
        }
    }

    /// Whether `conversation` was recorded in this project. Claude
    /// transcripts are filed under `projects/<encoded dir>/`, and worktrees
    /// of the project count as the project; Pi and OMP file sessions by
    /// their own naming, so their recorded working directory decides.
    pub fn contains(&self, conversation: &Conversation) -> bool {
        match conversation.source {
            Source::Claude => self.contains_claude_transcript(&conversation.path),
            Source::Pi | Source::Omp => conversation
                .project_path
                .as_deref()
                .or(conversation.cwd.as_deref())
                .is_some_and(|cwd| self.is_session_cwd(cwd)),
        }
    }

    /// A Claude transcript at `path` (`…/projects/<dir>/<uuid>.jsonl`).
    pub fn contains_claude_transcript(&self, path: &Path) -> bool {
        path.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| self.contains_claude_project(&name.to_string_lossy()))
    }

    /// A Claude project directory name (`-home-me-repo--worktrees-x`).
    pub fn contains_claude_project(&self, dir_name: &str) -> bool {
        is_same_project(dir_name, &self.dir_name)
    }

    /// A Pi/OMP session whose header records `cwd`.
    pub fn is_session_cwd(&self, cwd: &Path) -> bool {
        cwd == self.canonical || cwd.canonicalize().is_ok_and(|cwd| cwd == self.canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(source: Source, path: &str, cwd: Option<&Path>) -> Conversation {
        Conversation {
            origin: None,
            source,
            session_id: "session".to_owned(),
            path: PathBuf::from(path),
            index: 0,
            timestamp: chrono::Local::now(),
            preview: String::new(),
            preview_first: String::new(),
            preview_last: String::new(),
            full_text: String::new(),
            agent_search_text: String::new(),
            semantic_route_text: String::new(),
            semantic_turns: Vec::new(),
            semantic_turn_ranges: Vec::new(),
            search_text_lower: String::new(),
            dialogue_text_lower: String::new(),
            project_name: None,
            project_path: cwd.map(Path::to_path_buf),
            cwd: cwd.map(Path::to_path_buf),
            message_count: 1,
            parse_errors: Vec::new(),
            summary: None,
            custom_title: None,
            model: None,
            total_tokens: 0,
            duration_minutes: None,
        }
    }

    #[test]
    fn claude_transcripts_match_by_project_dir_including_worktrees() {
        let workspace = Workspace::at(Path::new("/code/repo"));
        let main = conversation(Source::Claude, "/c/projects/-code-repo/a.jsonl", None);
        let worktree = conversation(
            Source::Claude,
            "/c/projects/-code-repo--worktrees-fix/a.jsonl",
            None,
        );
        let other = conversation(Source::Claude, "/c/projects/-code-other/a.jsonl", None);

        assert!(workspace.contains(&main));
        assert!(workspace.contains(&worktree));
        assert!(!workspace.contains(&other));
    }

    #[test]
    fn pi_and_omp_sessions_match_by_recorded_cwd_not_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path().canonicalize().unwrap();
        let workspace = Workspace::at(&here);
        // Pi files sessions under `--<path>--`, which is not Claude's encoding.
        let pi_path = format!("/pi/sessions/--{}--/s.jsonl", here.display());
        for source in [Source::Pi, Source::Omp] {
            assert!(workspace.contains(&conversation(source, &pi_path, Some(&here))));
            assert!(!workspace.contains(&conversation(
                source,
                &pi_path,
                Some(Path::new("/somewhere/else"))
            )));
            assert!(!workspace.contains(&conversation(source, &pi_path, None)));
        }
    }
}
