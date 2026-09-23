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

/// Left gutter of every row line: a selection bar with padding.
pub const INDICATOR: &str = " ▌ ";
/// Columns left between the left part and the right-aligned metadata.
const MIN_PADDING: usize = 3;
/// Columns a preview or context line gives up to the indicator and margin.
const LINE_MARGIN: usize = 4;

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
    let msg_count = if conv.message_count == 1 {
        "1 msg".to_string()
    } else {
        format!("{} msgs", conv.message_count)
    };
    let duration = conv.duration_minutes.map(|m| {
        if m >= 60 {
            format!("{}h {}m", m / 60, m % 60)
        } else {
            format!("{}m", m)
        }
    });
    let semantic_meta = (source.semantic_mode && width >= 70)
        .then(|| source.semantic.map(semantic_row_metadata))
        .flatten();

    let widths = |part: &Option<String>| part.as_ref().map(|s| s.width() + 3).unwrap_or(0);
    let right_len =
        msg_count.width() + widths(&duration) + widths(&semantic_meta) + 3 + timestamp.width();
    let indicator_len = INDICATOR.width();
    let left_budget = width.saturating_sub(indicator_len + right_len + MIN_PADDING);

    let raw_project = conv
        .project_name
        .as_ref()
        .map(|name| {
            if source.multiple_sources {
                let label = conv
                    .origin
                    .as_deref()
                    .map_or(conv.source.list_label(), |origin| origin.label());
                format!("{label:<3} · {name}")
            } else {
                name.to_string()
            }
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

    let title_budget = left_budget.saturating_sub(project_len + 3);
    let custom_title = conv
        .custom_title
        .as_ref()
        .filter(|s| !s.is_empty() && title_budget > 4)
        .map(|s| format!(" · {}", simple_truncate(s, title_budget)));
    let custom_title_len = custom_title.as_ref().map(|s| s.width()).unwrap_or(0);

    let available_for_summary = width.saturating_sub(
        indicator_len + project_len + custom_title_len + right_len + MIN_PADDING + 4,
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
    let padding = width.saturating_sub(left_len + right_len + 1);

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
    use crate::history::{MessageRange, Source};
    use crate::semantic::types::{
        SemanticChunkIdentity, SemanticChunkSource, SemanticExplanation, SemanticQuality,
        SemanticScoreBreakdown,
    };
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn conversation() -> Conversation {
        Conversation {
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
        let row = project_row(&source(&conv, &matcher, 80), &RowEvidence::None, now());

        assert_eq!(row.project, "project");
        assert_eq!(row.custom_title.as_deref(), Some(" · a title"));
        assert_eq!(row.summary.as_deref(), Some(" · a summary"));
        assert_eq!(row.msg_count, "3 msgs");
        assert_eq!(row.duration.as_deref(), Some("1h 15m"));
        assert_eq!(
            (row.timestamp.as_str(), row.recency),
            ("yesterday", Recency::Days)
        );
        assert_eq!(row.preview, "visible preview text");
        assert_eq!(row.context, None);

        let left = INDICATOR.width()
            + row.project.width()
            + row.custom_title.as_deref().map_or(0, |s| s.width())
            + row.summary.as_deref().map_or(0, |s| s.width());
        let right = row.msg_count.width()
            + 3
            + row.duration.as_deref().map_or(0, |s| s.width())
            + 3
            + row.timestamp.width();
        assert_eq!(left + row.padding + right, 79, "one column of right margin");
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
