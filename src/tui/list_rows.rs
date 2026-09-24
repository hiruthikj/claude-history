//! What a list row says: the text of every part of a conversation row and
//! how it is budgeted into the available width. Nothing here knows about
//! styles or ratatui; `ui::render_list` colours what this module decides.
//!
//! Two kinds of work happen per row. Cheap fitting (truncation, budgets,
//! choosing which preview to show) runs in `project_row` on every frame.
//! The expensive part, scanning a conversation's `full_text` for matches the
//! preview hides, runs in `row_evidence` and is cached by `App` per result
//! set for the rows that are on screen, so the renderer never walks a
//! transcript.

use crate::history::Conversation;
use crate::search::QueryMatcher;
use crate::semantic::types::SemanticRationaleKind;
use crate::tui::app::SemanticResultMetadata;
use crate::tui::snippet::{context_snippet, fit_around_matches, sanitize_preview, simple_truncate};
use chrono::{DateTime, Local};
use unicode_width::UnicodeWidthStr;

/// Left gutter of the selected row's lines: a selection bar with padding.
/// Other rows get the same width in spaces, so only the selection is marked.
pub const INDICATOR: &str = " ▌ ";
/// Gap between the right-hand metadata columns.
pub const COLUMN_GAP: &str = "  ";
/// Right-hand columns are padded to these widths so they line up from row
/// to row: `999 msgs`, `23h 59m`, `Sep 15, 18:52`.
const MSG_COUNT_WIDTH: usize = 8;
const DURATION_WIDTH: usize = 7;
const TIMESTAMP_WIDTH: usize = 13;
/// Columns left between the left part and the right-aligned metadata.
const MIN_PADDING: usize = 3;
/// Columns a preview or context line gives up to the indicator and the
/// right-hand margin.
const LINE_MARGIN: usize = 5;
/// Blank columns kept at the right edge of every row line.
const RIGHT_MARGIN: usize = 2;
/// Narrowest list that still shows a conversation's duration.
const DURATION_MIN_WIDTH: usize = 100;

/// Everything a row is derived from.
#[derive(Clone, Copy)]
pub struct RowSource<'a> {
    pub conversation: &'a Conversation,
    pub matcher: &'a QueryMatcher,
    pub semantic_mode: bool,
    pub semantic: Option<&'a SemanticResultMetadata>,
    /// Width of the list area in columns.
    pub width: usize,
    /// Whether the corpus mixes Claude, Pi and OMP sessions (rows then carry a
    /// source label).
    pub multiple_sources: bool,
}

/// The `full_text` scans behind one row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RowEvidence {
    #[default]
    None,
    /// Query terms the stored preview hides, as byte ranges into `full_text`;
    /// a snippet around them replaces the preview line.
    Context(Vec<(usize, usize)>),
    /// Quoted literals missing from the fitted preview, as byte ranges into
    /// `full_text`; a snippet around them gets its own line.
    LiteralContext(Vec<(usize, usize)>),
}

/// Colour grading for the timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recency {
    Now,
    Minutes,
    Hours,
    Days,
    Old,
}

/// The text of one row, ready to be styled. Every string is already fitted
/// to the width it will occupy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    /// Project name, prefixed with the source label in a mixed corpus.
    pub project: String,
    /// Bytes of `project` that are the source label (styled as a badge).
    pub badge_len: usize,
    /// The untruncated project name, which picks the project's colour.
    pub hue_key: String,
    /// " · title", when the session was renamed and there is room.
    pub custom_title: Option<String>,
    /// " · summary", when the transcript has one and there is room.
    pub summary: Option<String>,
    /// Spaces between the left part and the right-aligned metadata.
    pub padding: usize,
    pub msg_count: String,
    /// Hybrid score in semantic mode on wide terminals.
    pub semantic_meta: Option<String>,
    pub duration: Option<String>,
    pub timestamp: String,
    pub recency: Recency,
    /// Second line: the preview, a hidden-context snippet, or the semantic
    /// evidence, fitted to the width.
    pub preview: String,
    /// Optional third line showing where a quoted literal matched.
    pub context: Option<String>,
}

/// Scan `full_text` for whatever the preview line will hide. Runs once per
/// result set per visible row; see `App::prepare_list_rows`.
pub fn row_evidence(source: &RowSource) -> RowEvidence {
    if source.matcher.is_empty() {
        return RowEvidence::None;
    }
    let conversation = source.conversation;
    if uses_lexical_context(source)
        && let Some(ranges) = source
            .matcher
            .hidden_context(&conversation.full_text, &conversation.preview)
    {
        return RowEvidence::Context(ranges);
    }
    let preview = fitted_preview(source, None);
    if source.matcher.literals_missing_from(&preview)
        && let Some(ranges) = source
            .matcher
            .literals_only()
            .hidden_context(&conversation.full_text, &preview)
    {
        return RowEvidence::LiteralContext(ranges);
    }
    RowEvidence::None
}

/// Lay out one row. `evidence` must come from `row_evidence` over the same
/// source (or be `RowEvidence::None` to skip context).
pub fn project_row(source: &RowSource, evidence: &RowEvidence, now: DateTime<Local>) -> ListRow {
    let conv = source.conversation;
    let width = source.width;

    let (timestamp, recency) = format_timestamp(conv.timestamp, now);
    let timestamp = format!("{timestamp:>TIMESTAMP_WIDTH$}");
    let msg_count = if conv.message_count == 1 {
        "1 msg".to_string()
    } else {
        format!("{} msgs", conv.message_count)
    };
    let msg_count = format!("{msg_count:>MSG_COUNT_WIDTH$}");
    // Below this width the title is worth more than the duration. Rows
    // without one keep the column blank so the timestamps still line up.
    let duration = (width >= DURATION_MIN_WIDTH).then(|| {
        let text = conv
            .duration_minutes
            .map(format_duration)
            .unwrap_or_default();
        format!("{text:>DURATION_WIDTH$}")
    });
    let semantic_meta = (source.semantic_mode && width >= 70)
        .then(|| source.semantic.map(semantic_row_metadata))
        .flatten();

    let gap = COLUMN_GAP.width();
    let widths = |part: &Option<String>| part.as_ref().map(|s| s.width() + gap).unwrap_or(0);
    let right_len =
        msg_count.width() + widths(&duration) + widths(&semantic_meta) + gap + timestamp.width();
    let indicator_len = INDICATOR.width();
    let left_budget = width.saturating_sub(indicator_len + right_len + MIN_PADDING + RIGHT_MARGIN);

    let badge = source.multiple_sources.then(|| {
        let label = conv
            .origin
            .as_deref()
            .map_or(conv.source.list_label(), |origin| origin.label());
        format!("{label:<3}")
    });
    let raw_project = project_label(conv)
        .map(|name| match &badge {
            Some(badge) => format!("{badge} · {name}"),
            None => name,
        })
        .unwrap_or_default();
    let has_title_or_summary = conv.custom_title.as_ref().is_some_and(|s| !s.is_empty())
        || conv.summary.as_ref().is_some_and(|s| !s.is_empty());
    // Narrow terminals keep some of the left budget for the title/summary so
    // a long project name cannot crowd them out entirely.
    let reserved_left_detail = if width < 90 && has_title_or_summary {
        (left_budget / 3).clamp(10, 24)
    } else {
        0
    };
    let project_budget = raw_project
        .width()
        .min(left_budget.saturating_sub(reserved_left_detail));
    let project = simple_truncate(&raw_project, project_budget);
    let project_len = project.width();
    let badge_len = badge.map_or(0, |badge| {
        if project.starts_with(&badge) {
            badge.len()
        } else {
            0
        }
    });

    let title_budget = left_budget.saturating_sub(project_len + 3);
    let custom_title = conv
        .custom_title
        .as_ref()
        .filter(|s| !s.is_empty() && title_budget > 4)
        .map(|s| format!(" · {}", simple_truncate(s, title_budget)));
    let custom_title_len = custom_title.as_ref().map(|s| s.width()).unwrap_or(0);

    let available_for_summary = width.saturating_sub(
        indicator_len + project_len + custom_title_len + right_len + MIN_PADDING + 3 + RIGHT_MARGIN,
    );
    let summary = conv
        .summary
        .as_ref()
        .filter(|s| !s.is_empty() && available_for_summary > 5)
        .map(|s| {
            if s.width() > available_for_summary {
                format!(" · {}", simple_truncate(s, available_for_summary))
            } else {
                format!(" · {}", s)
            }
        });

    let left_len = indicator_len
        + project_len
        + custom_title_len
        + summary.as_ref().map(|s| s.width()).unwrap_or(0);
    let padding = width.saturating_sub(left_len + right_len + RIGHT_MARGIN);

    let context_ranges = match evidence {
        RowEvidence::Context(ranges) => Some(ranges.as_slice()),
        _ => None,
    };
    let preview = fitted_preview(source, context_ranges);
    let context = match evidence {
        RowEvidence::LiteralContext(ranges) => {
            context_snippet(&conv.full_text, ranges, width.saturating_sub(LINE_MARGIN))
        }
        _ => None,
    };

    ListRow {
        project,
        badge_len,
        hue_key: hue_key(conv).to_string(),
        custom_title,
        summary,
        padding,
        msg_count,
        semantic_meta,
        duration,
        timestamp,
        recency,
        preview,
        context,
    }
}

/// Separates the launch folder from where the session moved to.
pub const MOVED_SEPARATOR: &str = " › ";

/// The project a row names: the launch folder, plus where the session
/// ended up when it moved (`Work › claude-history`).
pub fn project_label(conv: &Conversation) -> Option<String> {
    let name = conv.project_name.as_deref()?;
    Some(match moved_to(conv) {
        Some(moved) => format!("{name}{MOVED_SEPARATOR}{moved}"),
        None => name.to_string(),
    })
}

/// The name that picks a row's colour: where the work happened, so a
/// session that moved into a repo matches sessions started there.
pub fn hue_key(conv: &Conversation) -> std::borrow::Cow<'_, str> {
    match moved_to(conv) {
        Some(moved) => std::borrow::Cow::Owned(moved),
        None => std::borrow::Cow::Borrowed(conv.project_name.as_deref().unwrap_or_default()),
    }
}

fn moved_to(conv: &Conversation) -> Option<String> {
    let moved = crate::history::format_short_name_from_path(conv.last_cwd.as_deref()?);
    (Some(moved.as_str()) != conv.project_name.as_deref()).then_some(moved)
}

/// Lexical hidden-context applies unless a semantic result supplies its own
/// evidence for this row.
fn uses_lexical_context(source: &RowSource) -> bool {
    !source.semantic_mode || source.semantic.is_none()
}

/// The preview line: semantic evidence when the semantic ranking produced
/// this row, else a snippet around hidden lexical matches, else the stored
/// preview; fitted so query matches stay visible where that is meaningful.
fn fitted_preview(source: &RowSource, context_ranges: Option<&[(usize, usize)]>) -> String {
    let conv = source.conversation;
    let matcher = source.matcher;
    let max_len = source.width.saturating_sub(LINE_MARGIN);

    let lexical_context = context_ranges
        .filter(|_| uses_lexical_context(source))
        .and_then(|ranges| context_snippet(&conv.full_text, ranges, max_len));
    let semantic_preview = source
        .semantic
        .filter(|_| source.semantic_mode && !matcher.is_empty())
        .map(|metadata| sanitize_preview(&metadata.explanation.evidence_preview));
    let from_context = lexical_context.is_some();
    let text = semantic_preview
        .or(lexical_context)
        .unwrap_or_else(|| sanitize_preview(&conv.preview));

    if matcher.is_empty() {
        simple_truncate(&text, max_len)
    } else if source.semantic_mode && matcher.matches(&text) {
        fit_around_matches(&text, &matcher.ranges(&text), max_len)
    } else if source.semantic_mode || from_context {
        simple_truncate(&text, max_len)
    } else {
        fit_around_matches(&text, &matcher.ranges(&text), max_len)
    }
}

pub fn semantic_rationale_label(metadata: &SemanticResultMetadata) -> &'static str {
    match metadata.explanation.rationale_kind {
        SemanticRationaleKind::SemanticOnly => "semantic",
        SemanticRationaleKind::LexicalBoosted => "lex boost",
        SemanticRationaleKind::WeakMatch => "weak",
    }
}

fn semantic_row_metadata(metadata: &SemanticResultMetadata) -> String {
    format!("{:.2}", metadata.score_breakdown.hybrid)
}

/// Relative time for recent entries, absolute for older ones, with a
/// recency grade for colouring.
/// Session span at a glance: `45m`, `2h 5m`, `3h`, `8d 1h`. Minutes stop
/// mattering past a day, and `193h 15m` is not readable at a glance.
pub fn format_duration(minutes: u64) -> String {
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (days, hours, mins) {
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

pub fn format_timestamp(timestamp: DateTime<Local>, now: DateTime<Local>) -> (String, Recency) {
    let age = now.signed_duration_since(timestamp);

    // Future timestamps (clock skew): show absolute
    if age.num_seconds() < 0 {
        return (timestamp.format("%b %d, %H:%M").to_string(), Recency::Old);
    }

    let seconds = age.num_seconds();
    let minutes = age.num_minutes();
    let hours = age.num_hours();

    if seconds < 60 {
        return ("just now".to_string(), Recency::Now);
    }
    if minutes < 60 {
        return (format!("{minutes} min ago"), Recency::Minutes);
    }
    if hours < 24 {
        return (
            format!("{hours} hour{} ago", if hours == 1 { "" } else { "s" }),
            Recency::Hours,
        );
    }

    // Use calendar day difference for "yesterday" accuracy
    let day_diff = now
        .date_naive()
        .signed_duration_since(timestamp.date_naive())
        .num_days();
    if day_diff == 1 {
        return ("yesterday".to_string(), Recency::Days);
    }
    if day_diff < 7 {
        return (format!("{day_diff} days ago"), Recency::Days);
    }

    (timestamp.format("%b %d, %H:%M").to_string(), Recency::Old)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_at_a_glance() {
        assert_eq!(format_duration(0), "0m");
        assert_eq!(format_duration(45), "45m");
        assert_eq!(format_duration(125), "2h 5m");
        assert_eq!(format_duration(180), "3h");
        assert_eq!(format_duration(193 * 60 + 15), "8d 1h");
        assert_eq!(format_duration(2 * 1440 + 30), "2d");
    }
    use crate::history::{MessageRange, Source};
    use crate::semantic::types::{
        SemanticChunkIdentity, SemanticChunkSource, SemanticExplanation, SemanticQuality,
        SemanticScoreBreakdown,
    };
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn conversation() -> Conversation {
        Conversation {
            last_cwd: None,
            origin: None,
            source: Source::Claude,
            session_id: "session".to_owned(),
            path: PathBuf::from("/tmp/session.jsonl"),
            index: 0,
            timestamp: Local.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            preview: "visible preview text".to_string(),
            preview_first: String::new(),
            preview_last: String::new(),
            full_text: "visible preview text".to_string(),
            agent_search_text: String::new(),
            semantic_route_text: String::new(),
            semantic_turns: Vec::new(),
            semantic_turn_ranges: vec![MessageRange::single(1)],
            search_text_lower: String::new(),
            dialogue_text_lower: String::new(),
            project_name: Some("project".to_string()),
            project_path: None,
            cwd: None,
            message_count: 3,
            parse_errors: Vec::new(),
            summary: Some("a summary".to_string()),
            custom_title: Some("a title".to_string()),
            model: None,
            total_tokens: 0,
            duration_minutes: Some(75),
        }
    }

    fn semantic_metadata(evidence_preview: &str) -> SemanticResultMetadata {
        SemanticResultMetadata {
            score_breakdown: SemanticScoreBreakdown {
                hybrid: 0.5,
                semantic: 0.5,
                lexical: 0.0,
            },
            explanation: SemanticExplanation {
                rationale_kind: SemanticRationaleKind::SemanticOnly,
                quality: SemanticQuality::Strong,
                quality_label: "strong",
                matched_terms: Vec::new(),
                evidence_preview: evidence_preview.to_string(),
                chunk: SemanticChunkIdentity {
                    conversation_index: 0,
                    source: SemanticChunkSource::VisibleDialogue,
                    session: "test-session".to_string(),
                    chunk_index: 0,
                    message_range: MessageRange::single(1),
                },
            },
        }
    }

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap()
    }

    fn source<'a>(
        conv: &'a Conversation,
        matcher: &'a QueryMatcher,
        width: usize,
    ) -> RowSource<'a> {
        RowSource {
            conversation: conv,
            matcher,
            semantic_mode: false,
            semantic: None,
            width,
            multiple_sources: false,
        }
    }

    #[test]
    fn plain_row_fits_the_width_exactly() {
        let conv = conversation();
        let matcher = QueryMatcher::from_query("");
        let row = project_row(&source(&conv, &matcher, 120), &RowEvidence::None, now());

        assert_eq!(row.project, "project");
        assert_eq!(row.custom_title.as_deref(), Some(" · a title"));
        assert_eq!(row.summary.as_deref(), Some(" · a summary"));
        // Right-hand columns are padded so they line up across rows.
        assert_eq!(row.msg_count, "  3 msgs");
        assert_eq!(row.duration.as_deref(), Some(" 1h 15m"));
        assert_eq!(
            (row.timestamp.as_str(), row.recency),
            ("    yesterday", Recency::Days)
        );
        assert_eq!(row.preview, "visible preview text");
        assert_eq!(row.context, None);

        let left = INDICATOR.width()
            + row.project.width()
            + row.custom_title.as_deref().map_or(0, |s| s.width())
            + row.summary.as_deref().map_or(0, |s| s.width());
        let right = row.msg_count.width()
            + COLUMN_GAP.width()
            + row.duration.as_deref().map_or(0, |s| s.width())
            + COLUMN_GAP.width()
            + row.timestamp.width();
        assert_eq!(
            left + row.padding + right,
            120 - RIGHT_MARGIN,
            "right margin"
        );
    }

    #[test]
    fn a_session_that_moved_names_both_folders_and_colours_by_the_second() {
        let mut conv = conversation();
        conv.project_name = Some("Work".to_string());
        conv.last_cwd = Some(PathBuf::from("/home/me/Work/claude-history"));
        let matcher = QueryMatcher::from_query("");

        let row = project_row(&source(&conv, &matcher, 120), &RowEvidence::None, now());
        assert_eq!(row.project, "Work › claude-history");
        assert_eq!(row.hue_key, "claude-history");

        conv.last_cwd = Some(PathBuf::from("/elsewhere/Work"));
        let same_name = project_row(&source(&conv, &matcher, 120), &RowEvidence::None, now());
        assert_eq!(
            same_name.project, "Work",
            "no arrow when the name is the same"
        );
    }

    #[test]
    fn narrow_rows_give_the_duration_up_to_the_title() {
        let mut conv = conversation();
        conv.custom_title = Some("Last commit review for improvements".to_string());
        let matcher = QueryMatcher::from_query("");

        let narrow = project_row(&source(&conv, &matcher, 80), &RowEvidence::None, now());
        assert_eq!(narrow.duration, None);
        assert_eq!(
            narrow.custom_title.as_deref(),
            Some(" · Last commit review for improvements")
        );

        let wide = project_row(
            &source(&conv, &matcher, DURATION_MIN_WIDTH),
            &RowEvidence::None,
            now(),
        );
        assert_eq!(wide.duration.as_deref(), Some(" 1h 15m"));

        conv.duration_minutes = None;
        let blank = project_row(
            &source(&conv, &matcher, DURATION_MIN_WIDTH),
            &RowEvidence::None,
            now(),
        );
        assert_eq!(blank.duration.as_deref(), Some("       "));
    }

    #[test]
    fn source_label_is_added_only_for_mixed_corpora() {
        let conv = conversation();
        let matcher = QueryMatcher::from_query("");
        let mut src = source(&conv, &matcher, 80);
        src.multiple_sources = true;
        let row = project_row(&src, &RowEvidence::None, now());
        assert_eq!(row.project, "CC  · project");
    }

    #[test]
    fn narrow_rows_reserve_room_for_the_title() {
        let mut conv = conversation();
        conv.project_name = Some("claude-history/drop-semantic-feature-gate".to_string());
        let matcher = QueryMatcher::from_query("");
        let row = project_row(&source(&conv, &matcher, 70), &RowEvidence::None, now());
        assert!(row.project.ends_with('…'), "{:?}", row.project);
        assert!(row.custom_title.is_some(), "{row:?}");
    }

    #[test]
    fn hidden_word_match_replaces_the_preview_with_context() {
        let mut conv = conversation();
        conv.full_text = format!("visible preview text {} hiddenneedle", "x ".repeat(200));
        let matcher = QueryMatcher::from_query("hiddenneedle");
        let src = source(&conv, &matcher, 80);

        let evidence = row_evidence(&src);
        assert!(matches!(evidence, RowEvidence::Context(_)), "{evidence:?}");
        let row = project_row(&src, &evidence, now());
        assert!(row.preview.contains("hiddenneedle"), "{:?}", row.preview);
        assert_eq!(row.context, None);
    }

    #[test]
    fn hidden_literal_gets_its_own_context_line() {
        let mut conv = conversation();
        conv.full_text = format!("visible preview text {} hidden_literal", "x ".repeat(80));
        let matcher = QueryMatcher::from_query("\"hidden_literal\"");
        let src = source(&conv, &matcher, 80);

        let evidence = row_evidence(&src);
        // The full matcher also finds it, so it arrives as preview context.
        assert!(matches!(evidence, RowEvidence::Context(_)), "{evidence:?}");
        let row = project_row(&src, &evidence, now());
        assert!(row.preview.contains("hidden_literal"), "{:?}", row.preview);
    }

    #[test]
    fn semantic_rows_show_their_own_evidence_and_literal_context() {
        let mut conv = conversation();
        conv.full_text = format!("visible preview text {} hidden_literal", "x ".repeat(80));
        let matcher = QueryMatcher::from_query("\"hidden_literal\"");
        let metadata = semantic_metadata("semantic evidence sentence");
        let mut src = source(&conv, &matcher, 80);
        src.semantic_mode = true;
        src.semantic = Some(&metadata);

        let evidence = row_evidence(&src);
        assert!(
            matches!(evidence, RowEvidence::LiteralContext(_)),
            "{evidence:?}"
        );
        let row = project_row(&src, &evidence, now());
        assert_eq!(row.preview, "semantic evidence sentence");
        assert!(
            row.context
                .as_deref()
                .is_some_and(|c| c.contains("hidden_literal")),
            "{row:?}"
        );
        assert_eq!(row.semantic_meta.as_deref(), Some("0.50"));
    }

    #[test]
    fn empty_query_needs_no_evidence() {
        let conv = conversation();
        let matcher = QueryMatcher::from_query("");
        assert_eq!(
            row_evidence(&source(&conv, &matcher, 80)),
            RowEvidence::None
        );
    }

    #[test]
    fn timestamps_grade_by_age() {
        let base = Local.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
        let at = |secs: i64| base - chrono::Duration::seconds(secs);
        assert_eq!(
            format_timestamp(at(5), base),
            ("just now".into(), Recency::Now)
        );
        assert_eq!(
            format_timestamp(at(120), base),
            ("2 min ago".into(), Recency::Minutes)
        );
        assert_eq!(
            format_timestamp(at(3600), base),
            ("1 hour ago".into(), Recency::Hours)
        );
        assert_eq!(
            format_timestamp(at(36 * 3600), base),
            ("yesterday".into(), Recency::Days)
        );
        assert_eq!(
            format_timestamp(at(3 * 86400), base),
            ("3 days ago".into(), Recency::Days)
        );
        assert_eq!(format_timestamp(at(30 * 86400), base).1, Recency::Old);
        assert_eq!(
            format_timestamp(at(-60), base).1,
            Recency::Old,
            "clock skew"
        );
    }
}
