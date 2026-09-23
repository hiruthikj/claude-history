//! Where history comes from: the resolved list of session roots.
//!
//! With no `[[sources]]` in the config the list is today's implicit one:
//! `$CLAUDE_CONFIG_DIR` or `~/.claude`, plus the Pi and OMP roots their own
//! environment variables resolve to. A `[[sources]]` list replaces it
//! entirely, and may name the same kind more than once (e.g. a personal and a
//! work Claude config dir). Every loader, cache, delete and resume path takes
//! its roots from here, never from the environment directly.

use super::{Source, omp_loader, pi_loader};
use crate::error::{AppError, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One `[[sources]]` entry as written in the config file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub kind: Source,
    /// Claude: the config dir (holds `projects/`). Pi/OMP: the agent dir
    /// (holds `sessions/`); omitted means the kind's usual resolution.
    pub dir: Option<PathBuf>,
    /// Label in the list and the handle `--source` matches.
    pub name: Option<String>,
}

/// What resuming a session from a root does to the agent's config env var.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeEnv {
    /// Implicit root: the child sees whatever this process was started with.
    Inherit,
    Set(&'static str, PathBuf),
    /// Configured root that is the agent's own default: the variable must not
    /// leak in from this process and point the child somewhere else.
    Remove(&'static str),
}

impl ResumeEnv {
    pub fn apply(&self, command: &mut std::process::Command) {
        match self {
            Self::Inherit => {}
            Self::Set(name, value) => {
                command.env(name, value);
            }
            Self::Remove(name) => {
                command.env_remove(name);
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct SourceRoot {
    pub kind: Source,
    pub name: Option<String>,
    /// Claude: config dir. Pi/OMP: the sessions directory.
    pub dir: PathBuf,
    /// Pi/OMP: sessions sit directly in `dir` rather than per-project subdirs.
    pub flat: bool,
    pub resume_env: ResumeEnv,
}

impl SourceRoot {
    /// Name shown in list rows and status chips.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .unwrap_or_else(|| self.kind.list_label())
    }

    /// Claude transcripts live under `<config dir>/projects/<encoded cwd>/`.
    pub fn projects_dir(&self) -> PathBuf {
        match self.kind {
            Source::Claude => self.dir.join("projects"),
            Source::Pi | Source::Omp => self.dir.clone(),
        }
    }

    /// `--source` matches a name, or a kind (`claude`, `cc`, `pi`, `omp`).
    fn matches(&self, filter: &str) -> bool {
        self.name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(filter))
            || filter.eq_ignore_ascii_case(self.kind.label())
            || filter.eq_ignore_ascii_case(self.kind.list_label())
    }

    pub fn pi_root(&self) -> pi_loader::PiSessionRoot {
        pi_loader::PiSessionRoot {
            path: self.dir.clone(),
            flat: self.flat,
        }
    }

    pub fn omp_root(&self) -> omp_loader::OmpSessionRoot {
        omp_loader::OmpSessionRoot {
            path: self.dir.clone(),
            flat: self.flat,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceSet {
    roots: Vec<Arc<SourceRoot>>,
}

/// Environment the implicit roots are resolved from; a parameter so tests
/// need not touch the process environment.
#[derive(Clone, Debug, Default)]
pub struct SourceEnv {
    pub home: Option<PathBuf>,
    pub claude_config_dir: Option<PathBuf>,
}

impl SourceEnv {
    pub fn from_process() -> Self {
        Self {
            home: home::home_dir(),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        }
    }
}

impl SourceSet {
    /// The roots named by the config's `[[sources]]`, or the implicit ones.
    pub fn resolve(configured: Option<&[SourceConfig]>) -> Result<Self> {
        let env = SourceEnv::from_process();
        match configured {
            Some(entries) => Self::from_config(entries, &env),
            None => Ok(Self::implicit(&env)),
        }
    }

    /// Resolves from config, then narrows to `--source` filters.
    pub fn resolve_selected(
        configured: Option<&[SourceConfig]>,
        filters: &[String],
    ) -> Result<Self> {
        Self::resolve(configured)?.select(filters)
    }

    pub fn implicit(env: &SourceEnv) -> Self {
        let mut roots = Vec::new();
        let claude_dir = env
            .claude_config_dir
            .clone()
            .or_else(|| env.home.as_ref().map(|home| home.join(".claude")));
        if let Some(dir) = claude_dir {
            roots.push(SourceRoot {
                kind: Source::Claude,
                name: None,
                dir,
                flat: false,
                resume_env: ResumeEnv::Inherit,
            });
        }
        if let Ok(root) = pi_loader::session_root() {
            roots.push(SourceRoot {
                kind: Source::Pi,
                name: None,
                dir: root.path,
                flat: root.flat,
                resume_env: ResumeEnv::Inherit,
            });
        }
        if let Ok(root) = omp_loader::session_root() {
            roots.push(SourceRoot {
                kind: Source::Omp,
                name: None,
                dir: root.path,
                flat: root.flat,
                resume_env: ResumeEnv::Inherit,
            });
        }
        Self::from_roots(roots)
    }

    pub fn from_config(entries: &[SourceConfig], env: &SourceEnv) -> Result<Self> {
        if entries.is_empty() {
            return Err(config_error("[[sources]] is present but lists no sources"));
        }
        let mut roots: Vec<SourceRoot> = Vec::with_capacity(entries.len());
        for entry in entries {
            if let Some(name) = &entry.name
                && (name.trim().is_empty() || name.chars().any(char::is_whitespace))
            {
                return Err(config_error(&format!(
                    "source name {name:?} must be non-empty and contain no whitespace"
                )));
            }
            let root = configured_root(entry, env)?;
            if let Some(name) = &root.name
                && roots.iter().any(|other| {
                    other
                        .name
                        .as_deref()
                        .is_some_and(|other| other.eq_ignore_ascii_case(name))
                })
            {
                return Err(config_error(&format!("source name {name:?} is used twice")));
            }
            if roots
                .iter()
                .any(|other| other.kind == root.kind && same_dir(&other.dir, &root.dir))
            {
                return Err(config_error(&format!(
                    "{} source {} is listed twice",
                    root.kind.label(),
                    root.dir.display()
                )));
            }
            roots.push(root);
        }
        Ok(Self::from_roots(roots))
    }

    #[cfg(test)]
    pub fn from_roots_for_test(roots: Vec<SourceRoot>) -> Self {
        Self::from_roots(roots)
    }

    fn from_roots(roots: Vec<SourceRoot>) -> Self {
        Self {
            roots: roots.into_iter().map(Arc::new).collect(),
        }
    }

    /// Keeps the roots matching any filter; an empty filter keeps all. A
    /// filter that matches nothing is an error naming what would match.
    pub fn select(self, filters: &[String]) -> Result<Self> {
        if filters.is_empty() {
            return Ok(self);
        }
        for filter in filters {
            if !self.roots.iter().any(|root| root.matches(filter)) {
                let mut available = self
                    .roots
                    .iter()
                    .map(|root| root.label().to_string())
                    .collect::<Vec<_>>();
                available.dedup();
                return Err(config_error(&format!(
                    "unknown source {filter:?} (available: {})",
                    available.join(", ")
                )));
            }
        }
        let roots = self
            .roots
            .into_iter()
            .filter(|root| filters.iter().any(|filter| root.matches(filter)))
            .collect();
        Ok(Self { roots })
    }

    pub fn roots(&self) -> &[Arc<SourceRoot>] {
        &self.roots
    }

    pub fn of_kind(&self, kind: Source) -> impl Iterator<Item = &Arc<SourceRoot>> {
        self.roots.iter().filter(move |root| root.kind == kind)
    }

    /// The Claude root `--show-dir` and file-level fallbacks use: the first
    /// one listed.
    pub fn primary_claude(&self) -> Option<&Arc<SourceRoot>> {
        self.of_kind(Source::Claude).next()
    }

    /// The root that owns a transcript path, if any.
    pub fn owner_of(&self, path: &Path) -> Option<&Arc<SourceRoot>> {
        self.roots
            .iter()
            .filter(|root| path.starts_with(root.projects_dir()))
            .max_by_key(|root| root.projects_dir().components().count())
    }
}

fn configured_root(entry: &SourceConfig, env: &SourceEnv) -> Result<SourceRoot> {
    let home = env.home.as_deref();
    let dir = entry
        .dir
        .as_ref()
        .map(|dir| expand_home(dir, home))
        .transpose()?;
    let name = entry.name.clone();
    match entry.kind {
        Source::Claude => {
            let dir = dir.ok_or_else(|| config_error("a claude source needs `dir`"))?;
            let default = home.map(|home| home.join(".claude"));
            Ok(SourceRoot {
                kind: Source::Claude,
                name,
                resume_env: env_for(&dir, default.as_deref(), "CLAUDE_CONFIG_DIR"),
                dir,
                flat: false,
            })
        }
        Source::Pi => {
            let root = match &dir {
                Some(agent_dir) => pi_loader::session_root_for_agent_dir(agent_dir.clone())?,
                None => pi_loader::session_root()?,
            };
            let default = home.map(|home| home.join(".pi").join("agent"));
            Ok(SourceRoot {
                kind: Source::Pi,
                name,
                resume_env: dir.as_deref().map_or(ResumeEnv::Inherit, |dir| {
                    env_for(dir, default.as_deref(), "PI_CODING_AGENT_DIR")
                }),
                dir: root.path,
                flat: root.flat,
            })
        }
        Source::Omp => {
            let root = match &dir {
                Some(agent_dir) => omp_loader::session_root_for_agent_dir(agent_dir.clone())?,
                None => omp_loader::session_root()?,
            };
            let default = home.map(|home| home.join(".omp").join("agent"));
            Ok(SourceRoot {
                kind: Source::Omp,
                name,
                resume_env: dir.as_deref().map_or(ResumeEnv::Inherit, |dir| {
                    env_for(dir, default.as_deref(), "PI_CODING_AGENT_DIR")
                }),
                dir: root.path,
                flat: root.flat,
            })
        }
    }
}

/// A configured dir is passed to the resumed agent, except its own default,
/// which is reached by clearing the variable: Claude reads `~/.claude.json`
/// only when `CLAUDE_CONFIG_DIR` is unset, so setting it to `~/.claude` would
/// not be the same session environment.
fn env_for(dir: &Path, default: Option<&Path>, var: &'static str) -> ResumeEnv {
    if default.is_some_and(|default| same_dir(dir, default)) {
        ResumeEnv::Remove(var)
    } else {
        ResumeEnv::Set(var, dir.to_path_buf())
    }
}

fn same_dir(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    left == right || canonical(left) == canonical(right)
}

fn expand_home(path: &Path, home: Option<&Path>) -> Result<PathBuf> {
    let missing_home = || config_error("cannot expand `~` without a home directory");
    let expanded = if path == Path::new("~") {
        home.ok_or_else(missing_home)?.to_path_buf()
    } else if let Ok(rest) = path.strip_prefix("~") {
        home.ok_or_else(missing_home)?.join(rest)
    } else {
        path.to_path_buf()
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Err(config_error(&format!(
            "source dir {} must be absolute or start with ~/",
            path.display()
        )))
    }
}

fn config_error(message: &str) -> AppError {
    AppError::ConfigError(format!("[[sources]]: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(home: &Path) -> SourceEnv {
        SourceEnv {
            home: Some(home.to_path_buf()),
            claude_config_dir: None,
        }
    }

    fn claude(dir: &str, name: Option<&str>) -> SourceConfig {
        SourceConfig {
            kind: Source::Claude,
            dir: Some(PathBuf::from(dir)),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn implicit_claude_root_prefers_env_then_home() {
        let home = Path::new("/home/u");
        let set = SourceSet::implicit(&env(home));
        let claude = set.primary_claude().unwrap();
        assert_eq!(claude.dir, home.join(".claude"));
        assert_eq!(claude.resume_env, ResumeEnv::Inherit);

        let set = SourceSet::implicit(&SourceEnv {
            home: Some(home.to_path_buf()),
            claude_config_dir: Some(PathBuf::from("/cfg/work")),
        });
        assert_eq!(set.primary_claude().unwrap().dir, Path::new("/cfg/work"));
    }

    #[test]
    fn configured_claude_roots_expand_home_and_pick_resume_env() {
        let home = Path::new("/home/u");
        let set = SourceSet::from_config(
            &[
                claude("~/.claude", None),
                claude("~/.claude-work", Some("work")),
            ],
            &env(home),
        )
        .unwrap();
        let roots = set.roots();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].dir, home.join(".claude"));
        assert_eq!(
            roots[0].resume_env,
            ResumeEnv::Remove("CLAUDE_CONFIG_DIR"),
            "the default dir is reached by clearing the variable"
        );
        assert_eq!(roots[0].label(), "CC");
        assert_eq!(
            roots[1].resume_env,
            ResumeEnv::Set("CLAUDE_CONFIG_DIR", home.join(".claude-work"))
        );
        assert_eq!(roots[1].label(), "work");
        assert_eq!(
            roots[1].projects_dir(),
            home.join(".claude-work").join("projects")
        );
    }

    #[test]
    fn configured_sources_reject_bad_entries() {
        let home = Path::new("/home/u");
        let err = |entries: &[SourceConfig]| {
            SourceSet::from_config(entries, &env(home))
                .unwrap_err()
                .to_string()
        };
        assert!(err(&[]).contains("lists no sources"));
        assert!(
            err(&[SourceConfig {
                kind: Source::Claude,
                dir: None,
                name: None
            }])
            .contains("needs `dir`")
        );
        assert!(err(&[claude("relative/dir", None)]).contains("must be absolute"));
        assert!(err(&[claude("/a", Some("w")), claude("/b", Some("W"))]).contains("used twice"));
        assert!(err(&[claude("/a", None), claude("/a", Some("x"))]).contains("listed twice"));

        let real = tempfile::tempdir().unwrap();
        let link = real.path().parent().unwrap().join(format!(
            "{}-link",
            real.path().file_name().unwrap().to_string_lossy()
        ));
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        let symlinked = err(&[
            claude(&real.path().to_string_lossy(), None),
            claude(&link.to_string_lossy(), Some("x")),
        ]);
        std::fs::remove_file(&link).unwrap();
        assert!(symlinked.contains("listed twice"), "{symlinked}");
        assert!(err(&[claude("/a", Some("my work"))]).contains("no whitespace"));
    }

    #[test]
    fn select_matches_names_and_kinds_and_rejects_unknown() {
        let home = Path::new("/home/u");
        let set = SourceSet::from_config(
            &[claude("/a", Some("personal")), claude("/b", Some("work"))],
            &env(home),
        )
        .unwrap();

        let work = set.clone().select(&["WORK".to_string()]).unwrap();
        assert_eq!(work.roots().len(), 1);
        assert_eq!(work.roots()[0].label(), "work");

        assert_eq!(
            set.clone()
                .select(&["claude".to_string()])
                .unwrap()
                .roots()
                .len(),
            2
        );
        assert_eq!(set.clone().select(&[]).unwrap().roots().len(), 2);

        let error = set.select(&["nope".to_string()]).unwrap_err().to_string();
        assert!(error.contains("unknown source \"nope\""), "{error}");
        assert!(error.contains("personal, work"), "{error}");
    }

    #[test]
    fn owner_of_picks_the_root_holding_the_path() {
        let home = Path::new("/home/u");
        let set = SourceSet::from_config(
            &[
                claude("/c/main", None),
                claude("/c/main-work", Some("work")),
            ],
            &env(home),
        )
        .unwrap();
        let path = Path::new("/c/main-work/projects/-x/1.jsonl");
        assert_eq!(set.owner_of(path).unwrap().label(), "work");
        assert!(set.owner_of(Path::new("/elsewhere/1.jsonl")).is_none());
    }

    #[test]
    fn resume_env_sets_or_clears_the_agent_variable() {
        let mut command = std::process::Command::new("claude");
        ResumeEnv::Set("CLAUDE_CONFIG_DIR", PathBuf::from("/cfg/work")).apply(&mut command);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            vec![(
                std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"),
                Some(std::ffi::OsStr::new("/cfg/work"))
            )]
        );

        let mut command = std::process::Command::new("claude");
        ResumeEnv::Remove("CLAUDE_CONFIG_DIR").apply(&mut command);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            vec![(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"), None)]
        );

        let mut command = std::process::Command::new("claude");
        ResumeEnv::Inherit.apply(&mut command);
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn source_config_parses_from_toml() {
        let config: crate::config::ConfigFile = toml::from_str(
            r#"
[[sources]]
kind = "claude"
dir = "~/.claude"

[[sources]]
kind = "claude"
dir = "~/.claude-work"
name = "work"

[[sources]]
kind = "pi"
"#,
        )
        .unwrap();
        let sources = config.sources.unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[1].name.as_deref(), Some("work"));
        assert_eq!(sources[2].kind, Source::Pi);
        assert_eq!(sources[2].dir, None);

        let error = toml::from_str::<crate::config::ConfigFile>(
            "[[sources]]\nkind = \"codex\"\ndir = \"/x\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown variant"));
    }
}
