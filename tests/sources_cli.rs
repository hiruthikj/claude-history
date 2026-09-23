//! `[[sources]]`: several Claude config dirs merged into one corpus, with
//! `--source` narrowing every entry point. Runs the built binary under a
//! temporary `HOME` so the config file and caches are the test's own.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PERSONAL_UUID: &str = "11111111-1111-4111-8111-111111111111";
const WORK_UUID: &str = "22222222-2222-4222-8222-222222222222";

struct Fixture {
    home: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home");
        let fixture = Self { home };
        fixture.write_session(&fixture.personal(), PERSONAL_UUID, "personal");
        fixture.write_session(&fixture.work(), WORK_UUID, "work");
        let config_dir = fixture.home().join(".config/claude-history");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            r#"
[[sources]]
kind = "claude"
dir = "~/.claude"

[[sources]]
kind = "claude"
dir = "~/cfg-work"
name = "work"
"#,
        )
        .unwrap();
        fixture
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn personal(&self) -> PathBuf {
        self.home().join(".claude")
    }

    fn work(&self) -> PathBuf {
        self.home().join("cfg-work")
    }

    fn write_session(&self, config_dir: &Path, uuid: &str, which: &str) {
        let project = config_dir.join("projects").join("-tmp-sources-test");
        std::fs::create_dir_all(&project).unwrap();
        let user = serde_json::json!({
            "type": "user",
            "timestamp": "2026-07-20T00:00:00Z",
            "cwd": "/tmp/sources-test",
            "message": {"role": "user", "content": format!("sharedneedle from the {which} config")}
        });
        let assistant = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-07-20T00:00:01Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "answer"}]}
        });
        std::fs::write(
            project.join(format!("{uuid}.jsonl")),
            format!("{user}\n{assistant}\n"),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_claude-history"))
            .env("HOME", self.home())
            // An inherited value must not matter once [[sources]] is set.
            .env("CLAUDE_CONFIG_DIR", self.home().join("elsewhere"))
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("PI_CODING_AGENT_SESSION_DIR")
            .env_remove("XDG_DATA_HOME")
            .current_dir(self.home())
            .args(args)
            .output()
            .expect("run claude-history")
    }

    fn stdout(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

fn hit_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| line.starts_with("hit "))
        .collect()
}

#[test]
fn agent_search_merges_sources_and_labels_named_ones() {
    let fixture = Fixture::new();
    let output = fixture.stdout(&["agent", "search", "sharedneedle", "--flat"]);
    let hits = hit_lines(&output);

    assert_eq!(hits.len(), 2, "{output}");
    let work = hits
        .iter()
        .find(|line| line.contains(WORK_UUID))
        .expect("work hit");
    let personal = hits
        .iter()
        .find(|line| line.contains(PERSONAL_UUID))
        .expect("personal hit");
    assert!(work.contains(" origin=work "), "{work}");
    assert!(
        !personal.contains("origin="),
        "unnamed sources emit no origin atom: {personal}"
    );
}

#[test]
fn source_flag_narrows_agent_search_and_read() {
    let fixture = Fixture::new();
    let output = fixture.stdout(&[
        "agent",
        "search",
        "sharedneedle",
        "--flat",
        "--source",
        "work",
    ]);
    let hits = hit_lines(&output);
    assert_eq!(hits.len(), 1, "{output}");
    assert!(hits[0].contains(WORK_UUID));

    let outline = fixture.stdout(&["agent", "outline", WORK_UUID, "--source", "work"]);
    assert!(outline.contains("origin=work"), "{outline}");
    assert!(outline.contains("sharedneedle from the work config"));

    let missing = fixture.run(&["agent", "outline", PERSONAL_UUID, "--source", "work"]);
    assert!(!missing.status.success());
}

#[test]
fn unknown_source_is_an_error_naming_the_choices() {
    let fixture = Fixture::new();
    let output = fixture.run(&["agent", "search", "sharedneedle", "--source", "nope"]);
    assert!(!output.status.success());
    // Agent errors are one escaped protocol line on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("protocol agent-error"), "{stderr}");
    assert!(stderr.contains("unknown%20source%20%22nope%22"), "{stderr}");
    assert!(stderr.contains("CC%2C%20work"), "{stderr}");

    let output = fixture.run(&["--show-dir", "--source", "nope"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown source \"nope\""), "{stderr}");
}

#[test]
fn show_dir_and_delete_use_the_selected_source() {
    let fixture = Fixture::new();
    let shown = fixture.stdout(&["--show-dir", "--source", "work"]);
    assert!(
        Path::new(shown.trim()).starts_with(fixture.work().join("projects")),
        "{shown}"
    );

    let wrong_root = fixture.run(&[
        "--delete", WORK_UUID, "--source", "claude", "--source", "CC",
    ]);
    // `claude` matches both Claude roots, so the work copy is found.
    assert!(wrong_root.status.success());
    assert!(
        !fixture
            .work()
            .join(format!("projects/-tmp-sources-test/{WORK_UUID}.jsonl"))
            .exists()
    );

    let not_there = fixture.run(&["--delete", PERSONAL_UUID, "--source", "work"]);
    assert!(
        !not_there.status.success(),
        "a delete narrowed to another source must not find the session"
    );
    assert!(
        fixture
            .personal()
            .join(format!("projects/-tmp-sources-test/{PERSONAL_UUID}.jsonl"))
            .exists()
    );
}

#[test]
fn semantic_cache_generation_refuses_a_source_subset() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--generate-semantic-cache", "--source", "work"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be combined with --source"));
}

#[test]
fn within_target_record_carries_the_origin() {
    let fixture = Fixture::new();
    let output = fixture.stdout(&["agent", "within", WORK_UUID, "sharedneedle"]);
    let target = output
        .lines()
        .find(|line| line.starts_with("conversation "))
        .expect("target record");
    assert!(target.ends_with(" origin=work"), "{target}");
}

#[test]
fn caches_are_kept_per_source_dir() {
    let fixture = Fixture::new();
    fixture.stdout(&["agent", "search", "sharedneedle", "--flat"]);
    let cache = fixture.home().join(".cache/claude-history");
    assert!(
        cache.join("projects/-tmp-sources-test.bin").exists(),
        "~/.claude keeps the legacy cache path"
    );
    let namespaced = std::fs::read_dir(&cache)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("config-"))
        .count();
    assert_eq!(namespaced, 1, "the work dir gets its own cache namespace");
}
