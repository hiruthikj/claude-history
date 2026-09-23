use crate::config::KeyBindings;
use crate::search::QueryMatcher;
use crate::search::mode::SortMode;
use crate::tui::app::{
    App, AppMode, DialogMode, ListSearchMode, LoadingState, ViewSearchMode, ViewState,
    list_lines_per_item,
};
use crate::tui::list_layout::{self, ListLayout};
use crate::tui::list_rows::{
    INDICATOR, ListRow, Recency, RowSource, format_duration, project_row, row_evidence,
    semantic_rationale_label,
};
#[cfg(test)]
use crate::tui::snippet::fit_around_matches;
use crate::tui::snippet::{highlight, highlight_ranges, sanitize_preview, simple_truncate};
use crate::tui::theme::{self, Theme};
use crate::tui::viewer::{LineStyle, RenderedLine};
use chrono::Local;
use ratatui::layout::Position;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use std::borrow::Cow;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Get the current theme
fn th() -> &'static Theme {
    theme::detect_theme()
}

/// Convert theme RGB tuple to ratatui Color
fn rgb(c: (u8, u8, u8)) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Duration before status messages auto-clear
const STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// Format model name for display (e.g., "claude-opus-4-5-20251101" → "opus-4.5")
fn format_model_name(model: &str) -> String {
    // Handle claude-opus-4-5-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-opus-4-5-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "opus-4.5".to_string();
    }

    // Handle claude-sonnet-4-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-sonnet-4-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "sonnet-4".to_string();
    }

    // Handle claude-3-5-sonnet-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-3-5-sonnet-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "sonnet-3.5".to_string();
    }

    // Handle claude-3-5-haiku-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-3-5-haiku-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "haiku-3.5".to_string();
    }

    // Handle claude-3-opus-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-3-opus-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "opus-3".to_string();
    }

    // Handle claude-3-sonnet-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-3-sonnet-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "sonnet-3".to_string();
    }

    // Handle claude-3-haiku-YYYYMMDD format
    if let Some(rest) = model.strip_prefix("claude-3-haiku-")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return "haiku-3".to_string();
    }

    // Unknown format - truncate if too long
    if model.len() > 20 {
        format!("{}…", &model[..19])
    } else {
        model.to_string()
    }
}

/// Format token count with K/M suffix (short form, e.g., "926k")
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Format token count with K/M suffix and "tokens" label (long form, e.g., "926k tokens")
fn format_tokens_long(tokens: u64) -> String {
    format!("{} tokens", format_tokens(tokens))
}

/// Render the TUI
pub fn render(frame: &mut Frame, app: &App) {
    match app.app_mode() {
        AppMode::List => render_list_mode(frame, app),
        AppMode::View(state) => render_view_mode(frame, app, state),
    }
}

/// Render the list mode (conversation browser)
fn render_list_mode(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Outer border wrapping the entire app
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(th().border)));
    frame.render_widget(outer_block, area);

    let layout = ListLayout::new(
        area,
        list_lines_per_item(app.list_search_mode(), app.query()),
    );
    render_search_bar(frame, app, layout.search_bar);
    render_list(frame, app, layout.list);

    // Bottom bar (absent on tiny terminals): confirm dialog > status message > hotkeys
    if let Some(bottom) = layout.status_bar {
        if *app.dialog_mode() == DialogMode::ConfirmDelete {
            render_confirm_dialog(frame, bottom);
        } else if let Some((msg, instant)) = app.status_message()
            && instant.elapsed() < STATUS_TTL
        {
            render_status_message(frame, msg, bottom);
        } else {
            render_list_status_bar(frame, app, bottom);
        }
    }

    match app.dialog_mode() {
        DialogMode::Help { scroll } => render_help_overlay(
            frame,
            false,
            false,
            app.semantic_toggle_available(),
            app.keys(),
            *scroll,
        ),
        DialogMode::SemanticDebug => render_semantic_debug_popup(frame, app),
        DialogMode::Rename { input, cursor } => render_rename_dialog(frame, input, *cursor),
        _ => {}
    }
}

fn render_status_message(frame: &mut Frame, msg: &str, area: Rect) {
    let status_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(msg, Style::default().fg(Color::Yellow)),
    ]);
    let status = Paragraph::new(status_line).style(Style::default().bg(rgb(th().status_bar_bg)));
    frame.render_widget(status, area);
}

fn render_activity_status(frame: &mut Frame, msg: &str, area: Rect) {
    let status_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(msg, Style::default().fg(rgb(th().accent)).bold()),
    ]);
    let status = Paragraph::new(status_line).style(Style::default().bg(rgb(th().status_bar_bg)));
    frame.render_widget(status, area);
}

/// One status-bar hint: a key and what it does, or a key and the state it
/// toggles. `priority` orders what survives a narrow terminal (lower stays).
struct Hint {
    priority: u8,
    /// State toggles sit at the right edge, actions at the left.
    right: bool,
    spans: Vec<Span<'static>>,
}

impl Hint {
    fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum()
    }
}

const HINT_GAP: usize = 2;

/// Drops the highest-priority hints until the rest fit `width`, keeping the
/// original order of what remains.
fn fit_hints(mut hints: Vec<Hint>, width: usize) -> Vec<Hint> {
    let total = |hints: &[Hint]| {
        hints.iter().map(Hint::width).sum::<usize>() + HINT_GAP * hints.len().saturating_sub(1)
    };
    while total(&hints) > width {
        let Some(drop) = hints
            .iter()
            .enumerate()
            .max_by_key(|(index, hint)| (hint.priority, *index))
            .map(|(index, _)| index)
        else {
            break;
        };
        hints.remove(drop);
    }
    hints
}

/// Actions on the left, the list's toggled state on the right; when the
/// terminal is narrow the least useful hints go first rather than the line
/// being cut off mid-word.
fn render_list_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let is_loading = app.is_loading();

    let key_style = Style::default().fg(rgb(th().accent));
    let label_style = Style::default().fg(rgb(th().text_muted));
    let active_style = Style::default().fg(rgb(th().accent)).bold();
    // Dimmed styles for unavailable shortcuts during loading
    let dim_key_style = Style::default().fg(rgb(th().dim_key));
    let dim_label_style = Style::default().fg(rgb(th().dim_label));

    if let Some(status) = app.semantic_activity_status_text() {
        render_activity_status(frame, &status, area);
        return;
    }

    let (action_key, action_label) = if is_loading {
        (dim_key_style, dim_label_style)
    } else {
        (key_style, label_style)
    };
    let action = |priority: u8, key: String, label: &'static str| Hint {
        priority,
        right: false,
        spans: vec![
            Span::styled(key, action_key),
            Span::styled(format!(" {label}"), action_label),
        ],
    };
    // `active` marks a non-default state, drawn in the accent.
    let toggle =
        |priority: u8, key: String, name: &'static str, value: String, active: bool| Hint {
            priority,
            right: true,
            spans: vec![
                Span::styled(key, key_style),
                Span::styled(format!(" {name}\u{b7}"), label_style),
                Span::styled(value, if active { active_style } else { label_style }),
            ],
        };

    let keys = app.keys();
    let actions = vec![
        action(1, "Enter".to_string(), "open"),
        action(2, keys.resume.short_label(), "resume"),
        action(5, keys.fork.short_label(), "fork"),
        action(7, keys.rename.short_label(), "rename"),
        action(6, keys.delete.short_label(), "delete"),
    ];

    let mut states = Vec::new();
    if app.has_project_context() {
        let project = app.workspace_filter();
        states.push(toggle(
            3,
            "Tab".to_string(),
            "scope",
            if project { "project" } else { "all" }.to_string(),
            project,
        ));
    }
    if app.has_source_choice() {
        let label = app.source_filter_label();
        states.push(toggle(
            3,
            "S-Tab".to_string(),
            "source",
            label.unwrap_or("all").to_string(),
            label.is_some(),
        ));
    }
    if app.semantic_toggle_available() {
        let semantic = app.list_search_mode() == ListSearchMode::Semantic;
        states.push(toggle(
            4,
            "^T".to_string(),
            "search",
            app.list_search_mode().label().to_string(),
            semantic,
        ));
    }
    let newest = app.list_sort() == SortMode::Recency;
    states.push(toggle(
        3,
        keys.sort.short_label(),
        "sort",
        if newest { "newest" } else { "best" }.to_string(),
        newest,
    ));
    states.push(Hint {
        priority: 0,
        right: true,
        spans: vec![
            Span::styled("?", key_style),
            Span::styled(" help", label_style),
        ],
    });

    render_hint_bar(frame, actions.into_iter().chain(states).collect(), area);
}

fn render_semantic_debug_popup(frame: &mut Frame, app: &App) {
    let Some(metadata) = app.semantic_result_metadata_for_selection() else {
        return;
    };
    let area = frame.area();
    let popup = centered_modal_area(area, 68, 10);
    frame.render_widget(Clear, popup);
    let background = Block::default().style(Style::default().bg(rgb(th().overlay_bg)));
    frame.render_widget(background, popup);
    let block = Block::default()
        .title(" Semantic result ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(th().accent)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.is_empty() {
        return;
    }

    let score = &metadata.score_breakdown;
    let explanation = &metadata.explanation;
    let lines = vec![
        Line::from(vec![
            Span::styled(" hybrid ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                format!("{:.2}", score.hybrid),
                Style::default().fg(rgb(th().accent)).bold(),
            ),
            Span::styled("  semantic ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                format!("{:.2}", score.semantic),
                Style::default().fg(rgb(th().text_primary)),
            ),
            Span::styled("  lexical ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                format!("{:.2}", score.lexical),
                Style::default().fg(rgb(th().text_primary)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" rationale ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                semantic_rationale_label(metadata),
                Style::default().fg(rgb(th().text_primary)),
            ),
            Span::styled("  quality ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                explanation.quality_label,
                Style::default().fg(rgb(th().text_primary)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" chunk ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                format!(
                    "{} #{}",
                    explanation.chunk.session, explanation.chunk.chunk_index
                ),
                Style::default().fg(rgb(th().text_primary)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" terms ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                if explanation.matched_terms.is_empty() {
                    "(none)".to_string()
                } else {
                    explanation.matched_terms.join(", ")
                },
                Style::default().fg(rgb(th().text_primary)),
            ),
        ]),
        Line::from(vec![
            Span::styled(" preview ", Style::default().fg(rgb(th().text_muted))),
            Span::styled(
                simple_truncate(
                    &sanitize_preview(&explanation.evidence_preview),
                    inner.width.saturating_sub(10) as usize,
                ),
                Style::default().fg(rgb(th().preview)),
            ),
        ]),
        Line::from(""),
        Line::styled(" Esc close", Style::default().fg(rgb(th().text_muted))),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One piece of the viewer header's metadata line; the kind picks its style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderPart {
    Project,
    Title,
    Model,
    Count,
    Duration,
    Tokens,
    Timestamp,
}

const HEADER_INDENT: &str = "  ";
const HEADER_SEPARATOR: &str = " \u{b7} ";

/// What the viewer header says at a given width. Both the layout (its
/// height) and the renderer read this, so they cannot disagree.
#[derive(Debug)]
struct ViewHeader {
    parts: Vec<(HeaderPart, String)>,
    summary: Option<String>,
    /// The summary fits after the metadata on the first line.
    summary_inline: bool,
}

impl ViewHeader {
    fn new(
        conv: Option<&crate::history::Conversation>,
        fallback: &std::path::Path,
        width: u16,
    ) -> Self {
        let Some(conv) = conv else {
            let stem = fallback
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("Unknown");
            return Self {
                parts: vec![(HeaderPart::Project, stem.to_string())],
                summary: None,
                summary_inline: false,
            };
        };
        let width = width as usize;
        let mut parts = vec![(
            HeaderPart::Project,
            conv.project_name
                .as_deref()
                .unwrap_or("Unknown")
                .to_string(),
        )];
        if let Some(title) = &conv.custom_title {
            parts.push((HeaderPart::Title, title.clone()));
        }
        if let Some(model) = &conv.model {
            parts.push((HeaderPart::Model, format_model_name(model)));
        }
        parts.push((
            HeaderPart::Count,
            match conv.message_count {
                1 => "1 message".to_string(),
                n => format!("{n} messages"),
            },
        ));
        if let Some(minutes) = conv.duration_minutes {
            parts.push((HeaderPart::Duration, format_duration(minutes)));
        }
        let timestamp = conv.timestamp.format("%Y-%m-%d %H:%M").to_string();
        let without_tokens = header_width(&parts) + separated(&timestamp);
        if conv.total_tokens > 0 {
            let long = format_tokens_long(conv.total_tokens);
            let tokens = if without_tokens + separated(&long) <= width {
                long
            } else {
                format_tokens(conv.total_tokens)
            };
            parts.push((HeaderPart::Tokens, tokens));
        }
        parts.push((HeaderPart::Timestamp, timestamp));
        let summary_inline = conv
            .summary
            .as_deref()
            .is_some_and(|summary| header_width(&parts) + separated(summary) <= width);
        Self {
            parts,
            summary: conv.summary.clone(),
            summary_inline,
        }
    }

    /// Metadata line, optional summary line, bottom border.
    fn height(&self) -> u16 {
        if self.summary.is_some() && !self.summary_inline {
            3
        } else {
            2
        }
    }
}

fn separated(text: &str) -> usize {
    HEADER_SEPARATOR.width() + text.width()
}

fn header_width(parts: &[(HeaderPart, String)]) -> usize {
    HEADER_INDENT.width()
        + parts
            .iter()
            .enumerate()
            .map(|(index, (_, text))| {
                if index == 0 {
                    text.width()
                } else {
                    separated(text)
                }
            })
            .sum::<usize>()
}

fn view_header(app: &App, state: &ViewState, width: u16) -> ViewHeader {
    let conv = app
        .conversations()
        .iter()
        .find(|c| c.path == state.conversation_path);
    ViewHeader::new(conv, &state.conversation_path, width)
}

#[derive(Clone, Copy, Debug)]
pub struct ViewLayoutRects {
    pub header: Rect,
    pub content: Rect,
    pub status: Rect,
}

pub fn view_layout_rects(area: Rect, app: &App, state: &ViewState) -> ViewLayoutRects {
    let status_height = if state.search_mode == ViewSearchMode::Typing {
        2
    } else {
        1
    };
    let header_height = view_header(app, state, area.width).height();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(status_height),
        ])
        .split(area);

    ViewLayoutRects {
        header: chunks[0],
        content: chunks[1],
        status: chunks[2],
    }
}

/// Render the view mode (conversation viewer)
fn render_view_mode(frame: &mut Frame, app: &App, state: &ViewState) {
    let layout = view_layout_rects(frame.area(), app, state);

    render_view_header(frame, app, state, layout.header);
    render_view_content(frame, state, layout.content);

    if state.search_mode == ViewSearchMode::Typing {
        render_search_input(frame, state, layout.status);
    } else {
        render_view_status_bar(frame, app, state, layout.status);
    }

    // Render dialog overlay if active
    match app.dialog_mode() {
        DialogMode::ConfirmDelete => render_confirm_dialog(frame, layout.status),
        DialogMode::ExportMenu { selected } => render_export_menu(frame, *selected, false),
        DialogMode::YankMenu { selected } => render_export_menu(frame, *selected, true),
        DialogMode::Help { scroll } => {
            render_help_overlay(
                frame,
                true,
                app.is_single_file_mode(),
                false,
                app.keys(),
                *scroll,
            );
        }
        DialogMode::SemanticDebug => render_semantic_debug_popup(frame, app),
        DialogMode::Rename { input, cursor } => render_rename_dialog(frame, input, *cursor),
        DialogMode::None => {}
    }
}

fn render_view_header(frame: &mut Frame, app: &App, state: &ViewState, area: Rect) {
    let header = view_header(app, state, area.width);
    let summary_style = Style::default().fg(rgb(th().header_summary));
    let mut spans = vec![Span::raw(HEADER_INDENT)];
    for (index, (part, text)) in header.parts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(HEADER_SEPARATOR));
        }
        let style = match part {
            HeaderPart::Project => Style::default().fg(rgb(th().project_color(text))).bold(),
            HeaderPart::Title => Style::default().fg(rgb(th().custom_title)),
            HeaderPart::Model => Style::default().fg(rgb(th().model_color)),
            HeaderPart::Duration => Style::default().fg(rgb(th().duration_color)),
            HeaderPart::Count | HeaderPart::Tokens | HeaderPart::Timestamp => {
                Style::default().fg(rgb(th().text_secondary))
            }
        };
        spans.push(Span::styled(text.clone(), style));
    }
    let mut lines = Vec::new();
    match header.summary {
        Some(summary) if header.summary_inline => {
            spans.push(Span::raw(HEADER_SEPARATOR));
            spans.push(Span::styled(summary, summary_style));
            lines.push(Line::from(spans));
        }
        Some(summary) => {
            lines.push(Line::from(spans));
            lines.push(Line::from(vec![
                Span::raw(HEADER_INDENT),
                Span::styled(summary, summary_style),
            ]));
        }
        None => lines.push(Line::from(spans)),
    }

    let header = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(rgb(th().border))),
    );

    frame.render_widget(header, area);
}

fn render_view_content(frame: &mut Frame, state: &ViewState, area: Rect) {
    let visible_height = area.height as usize;
    let query_lower = state.search_query.to_lowercase();

    // Determine focused message line range (only when nav mode active)
    let focused_range = if state.message_nav_active {
        state
            .focused_message
            .and_then(|idx| state.message_ranges.get(idx))
            .map(|m| m.start_line..m.end_line)
    } else {
        None
    };

    let visible_lines: Vec<Line> = state
        .rendered_lines
        .iter()
        .enumerate()
        .skip(state.scroll_offset)
        .take(visible_height)
        .map(|(line_idx, rendered)| {
            let is_current_match = state.search_matches.get(state.current_match) == Some(&line_idx);
            let has_match = !query_lower.is_empty() && state.search_matches.contains(&line_idx);

            let is_focused = focused_range
                .as_ref()
                .is_some_and(|r| r.contains(&line_idx));

            // Gutter indicator (only shown in message nav mode)
            let gutter = if state.message_nav_active {
                if is_focused {
                    Span::styled("▌ ", Style::default().fg(rgb(th().accent)))
                } else {
                    Span::raw("  ")
                }
            } else {
                Span::raw("")
            };

            let mut spans: Vec<Span> = vec![gutter];

            if has_match && !query_lower.is_empty() {
                spans.extend(highlight_line_matches(
                    rendered,
                    &query_lower,
                    is_current_match,
                ));
            } else {
                spans.extend(
                    rendered
                        .spans
                        .iter()
                        .map(|(text, style)| styled_span(text, style)),
                );
            }

            let is_hovered = rendered
                .tool_output_id
                .as_ref()
                .is_some_and(|id| state.hovered_tool_output.as_ref() == Some(id));
            if is_hovered {
                let used_width: usize = spans
                    .iter()
                    .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                    .sum();
                let padding = (area.width as usize).saturating_sub(used_width);
                if padding > 0 {
                    spans.push(Span::styled(
                        " ".repeat(padding),
                        Style::default().bg(rgb(th().selection_bg)),
                    ));
                }
            }

            let mut line = Line::from(spans);
            if is_hovered {
                line = line.style(Style::default().bg(rgb(th().selection_bg)));
            }

            line
        })
        .collect();

    let content = Paragraph::new(visible_lines);
    frame.render_widget(content, area);
}

fn render_view_status_bar(frame: &mut Frame, app: &App, state: &ViewState, area: Rect) {
    // Check for status message first
    if let Some((msg, instant)) = app.status_message()
        && instant.elapsed() < STATUS_TTL
    {
        let status_line = Line::from(vec![
            Span::raw("  "),
            Span::styled(msg, Style::default().fg(Color::Green)),
        ]);
        let status =
            Paragraph::new(status_line).style(Style::default().bg(rgb(th().status_bar_bg)));
        frame.render_widget(status, area);
        return;
    }

    // Fixed-width scroll position to prevent bar from jumping
    // Use minimum width of 4 for both numbers to handle most conversations
    let total = state.total_lines.max(1);
    let width = total.to_string().len().max(4);
    let scroll_pos = format!("[{:>width$}/{:<width$}]", state.scroll_offset + 1, total);

    let key_style = Style::default().fg(rgb(th().accent));
    let label_style = Style::default().fg(rgb(th().text_muted));
    let active_style = Style::default().fg(rgb(th().accent)).bold();
    let hint = |priority: u8, right: bool, key: String, label: String| Hint {
        priority,
        right,
        spans: vec![
            Span::styled(key, key_style),
            Span::styled(format!(" {label}"), label_style),
        ],
    };
    let toggle =
        |priority: u8, key: &'static str, name: &'static str, value: &str, active: bool| Hint {
            priority,
            right: true,
            spans: vec![
                Span::styled(key, key_style),
                Span::styled(format!(" {name}\u{b7}"), label_style),
                Span::styled(
                    value.trim().to_string(),
                    if active { active_style } else { label_style },
                ),
            ],
        };

    let mut hints = vec![Hint {
        priority: 0,
        right: false,
        spans: vec![Span::styled(
            scroll_pos,
            Style::default().fg(rgb(th().text_secondary)),
        )],
    }];
    if state.search_mode == ViewSearchMode::Active && !state.search_matches.is_empty() {
        hints.push(Hint {
            priority: 0,
            right: false,
            spans: vec![
                Span::styled("n/N", key_style),
                Span::styled(" match ", label_style),
                Span::styled(
                    format!("{}/{}", state.current_match + 1, state.search_matches.len()),
                    active_style,
                ),
                Span::styled(
                    format!(
                        " \u{201c}{}\u{201d}",
                        simple_truncate(&state.search_query, 24)
                    ),
                    label_style,
                ),
            ],
        });
        hints.push(hint(1, false, "Esc".into(), "clear".into()));
        // Opening a search hit lands here, and resuming is what comes next.
        hints.push(hint(
            2,
            false,
            app.keys().resume.short_label(),
            "resume".into(),
        ));
    } else {
        hints.extend([
            hint(1, false, "/".into(), "search".into()),
            hint(4, false, "{ }".into(), "prompt".into()),
            hint(4, false, "e".into(), "export".into()),
            hint(4, false, "y".into(), "yank".into()),
            hint(2, false, app.keys().resume.short_label(), "resume".into()),
            hint(5, false, app.keys().fork.short_label(), "fork".into()),
            hint(6, false, app.keys().delete.short_label(), "delete".into()),
            hint(3, false, "q".into(), "back".into()),
        ]);
    }
    let tools = state.tool_display.status_label();
    hints.extend([
        toggle(
            3,
            "t",
            "tools",
            tools,
            state.tool_display != crate::tui::ToolDisplayMode::Hidden,
        ),
        toggle(
            3,
            "T",
            "thinking",
            if state.show_thinking { "on" } else { "off" },
            state.show_thinking,
        ),
        toggle(
            5,
            "i",
            "timing",
            if state.show_timing { "on" } else { "off" },
            state.show_timing,
        ),
        hint(0, true, "?".into(), "help".into()),
    ]);

    render_hint_bar(frame, hints, area);
}

/// Lays hints out on one status line: left-side hints from the left edge,
/// right-side ones against the right, dropping by priority to fit.
fn render_hint_bar(frame: &mut Frame, hints: Vec<Hint>, area: Rect) {
    let margin = 2;
    let budget = (area.width as usize).saturating_sub(margin * 2 + HINT_GAP);
    let (right, left): (Vec<_>, Vec<_>) = fit_hints(hints, budget)
        .into_iter()
        .partition(|hint| hint.right);
    let join = |hints: Vec<Hint>| {
        let mut spans = Vec::new();
        for (index, hint) in hints.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" ".repeat(HINT_GAP)));
            }
            spans.extend(hint.spans);
        }
        spans
    };
    let left = join(left);
    let right = join(right);
    let used: usize = left
        .iter()
        .chain(&right)
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    let padding = (area.width as usize).saturating_sub(used + margin * 2);

    let mut spans = vec![Span::raw(" ".repeat(margin))];
    spans.extend(left);
    spans.push(Span::raw(" ".repeat(padding)));
    spans.extend(right);
    let status =
        Paragraph::new(Line::from(spans)).style(Style::default().bg(rgb(th().status_bar_bg)));
    frame.render_widget(status, area);
}

fn render_search_input(frame: &mut Frame, state: &ViewState, area: Rect) {
    let match_info = if state.search_matches.is_empty() {
        if state.search_query.is_empty() {
            String::new()
        } else {
            " (no matches)".to_string()
        }
    } else {
        format!(
            " ({}/{})",
            state.current_match + 1,
            state.search_matches.len()
        )
    };

    let input_line = Line::from(vec![
        Span::raw("  /"),
        Span::styled(
            &state.search_query,
            Style::default().fg(rgb(th().text_primary)),
        ),
        Span::styled(match_info, Style::default().fg(rgb(th().text_secondary))),
    ]);

    let input = Paragraph::new(input_line).style(Style::default().bg(rgb(th().status_bar_bg)));
    frame.render_widget(input, area);

    // Position cursor (account for "  /" prefix = 3 columns)
    let query_width: usize = state
        .search_query
        .chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    let max_x = area.x + area.width.saturating_sub(1);
    let cursor_x = (area.x + 3 + query_width.min(u16::MAX as usize) as u16).min(max_x);
    frame.set_cursor_position(Position::new(cursor_x, area.y));
}

/// Highlight search matches across the full line text, handling matches that span
/// across multiple styled spans. Works by finding match positions in the concatenated
/// line text, then rebuilding spans with highlights applied at the correct positions.
fn highlight_line_matches(
    rendered: &RenderedLine,
    query: &str,
    is_current_match: bool,
) -> Vec<Span<'static>> {
    // Concatenate all span texts to get the full line
    let full_text: String = rendered
        .spans
        .iter()
        .map(|(text, _)| text.as_str())
        .collect();
    let full_lower = full_text.to_lowercase();

    // Find match positions using char indices to safely handle Unicode
    // (lowercasing can change byte lengths for some characters)
    let orig_chars: Vec<(usize, char)> = full_text.char_indices().collect();
    let lower_chars: Vec<char> = full_lower.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();

    let mut match_byte_ranges: Vec<(usize, usize)> = Vec::new();
    if !query_chars.is_empty() {
        let mut i = 0;
        while i + query_chars.len() <= lower_chars.len() {
            if lower_chars[i..i + query_chars.len()] == query_chars[..] {
                // Guard against Unicode casing expansion (e.g. ß → ss) where
                // lower_chars may be longer than orig_chars
                if i >= orig_chars.len() {
                    break;
                }
                let start_byte = orig_chars[i].0;
                let end_byte = if i + query_chars.len() < orig_chars.len() {
                    orig_chars[i + query_chars.len()].0
                } else {
                    full_text.len()
                };
                match_byte_ranges.push((start_byte, end_byte));
                i += query_chars.len();
            } else {
                i += 1;
            }
        }
    }

    if match_byte_ranges.is_empty() {
        return rendered
            .spans
            .iter()
            .map(|(t, s)| styled_span(t, s))
            .collect();
    }

    let match_style = if is_current_match {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default()
            .bg(rgb(th().search_match_bg))
            .fg(Color::Black)
    };

    // Build output spans by walking through original spans and splitting at match boundaries
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut match_idx = 0;
    let mut global_offset: usize = 0;

    for (text, style) in &rendered.spans {
        let span_start = global_offset;
        let span_end = global_offset + text.len();
        let base_style = build_style(style);
        let mut pos = span_start;

        while pos < span_end {
            // Skip past matches that are entirely before our position
            while match_idx < match_byte_ranges.len() && match_byte_ranges[match_idx].1 <= pos {
                match_idx += 1;
            }

            if match_idx < match_byte_ranges.len() {
                let (ms, me) = match_byte_ranges[match_idx];
                if pos >= ms && pos < me {
                    // Inside a match
                    let end = me.min(span_end);
                    result.push(Span::styled(full_text[pos..end].to_string(), match_style));
                    pos = end;
                } else if ms < span_end {
                    // There's a match starting within this span, emit text before it
                    let end = ms.min(span_end);
                    if end > pos {
                        result.push(Span::styled(full_text[pos..end].to_string(), base_style));
                    }
                    pos = end;
                } else {
                    // No more matches in this span
                    result.push(Span::styled(
                        full_text[pos..span_end].to_string(),
                        base_style,
                    ));
                    pos = span_end;
                }
            } else {
                // No more matches at all
                result.push(Span::styled(
                    full_text[pos..span_end].to_string(),
                    base_style,
                ));
                pos = span_end;
            }
        }

        global_offset = span_end;
    }

    result
}

fn build_style(style: &LineStyle) -> Style {
    let mut s = Style::default();
    if let Some((r, g, b)) = style.fg {
        s = s.fg(Color::Rgb(r, g, b));
    }
    if style.bold {
        s = s.bold();
    }
    if style.italic {
        s = s.italic();
    }
    if style.dimmed {
        s = s.fg(rgb(th().text_muted));
    }
    s
}

fn styled_span(text: &str, style: &LineStyle) -> Span<'static> {
    Span::styled(text.to_string(), build_style(style))
}

fn render_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    let count_text = match app.loading_state() {
        LoadingState::Loading { loaded } => format!("Loading... {}", loaded),
        LoadingState::Ready => match app.selected() {
            Some(selected) => format!("{}/{}", selected + 1, app.filtered().len()),
            None => format!("0/{}", app.filtered().len()),
        },
    };
    let status_text = if app.list_search_mode() == ListSearchMode::Semantic {
        app.semantic_status_text()
            .map(|status| {
                format!(
                    "{} {} {}",
                    app.list_search_mode().label(),
                    count_text,
                    status
                )
            })
            .unwrap_or_else(|| format!("{} {}", app.list_search_mode().label(), count_text))
    } else {
        count_text
    };

    let prompt_style = Style::default().fg(rgb(th().accent));
    let (prompt_spans, prefix_width) = if app.workspace_filter() {
        (
            vec![
                Span::raw(" "),
                Span::styled("Project", Style::default().fg(rgb(th().text_muted))),
                Span::raw(" "),
                Span::styled("\u{276F} ", prompt_style),
            ],
            11,
        )
    } else {
        (
            vec![Span::raw(" "), Span::styled("\u{276F} ", prompt_style)],
            3,
        )
    };

    let status_style = if app.is_loading() {
        Style::default().fg(rgb(th().accent))
    } else {
        Style::default().fg(rgb(th().text_muted))
    };
    let available = area.width as usize;
    let min_gap = usize::from(available > prefix_width);
    let right_budget = available.saturating_sub(prefix_width + min_gap);
    let rendered_status = simple_truncate(&status_text, right_budget);
    let right_width =
        UnicodeWidthStr::width(rendered_status.as_str()) + usize::from(!rendered_status.is_empty());
    let query_budget = available.saturating_sub(prefix_width + right_width + min_gap);
    // An empty box says what it takes; the cursor still sits at its start.
    let (rendered_query, query_style) = if app.query().is_empty() {
        (
            simple_truncate(
                "search conversations · \"quoted\" = exact phrase · ? for keys",
                query_budget,
            ),
            Style::default().fg(rgb(th().dim_label)),
        )
    } else {
        (simple_truncate(app.query(), query_budget), Style::default())
    };
    let query_width = UnicodeWidthStr::width(rendered_query.as_str());
    let padding = available.saturating_sub(prefix_width + query_width + right_width);

    let mut spans = prompt_spans;
    spans.extend([
        Span::styled(rendered_query, query_style),
        Span::raw(" ".repeat(padding)),
        Span::styled(rendered_status, status_style),
        Span::raw(" "),
    ]);
    let search_line = Line::from(spans);

    let input = Paragraph::new(search_line).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(rgb(th().border))),
    );

    frame.render_widget(input, area);

    if area.width > prefix_width as u16 {
        let cursor_offset: u16 = app
            .query()
            .chars()
            .take(app.cursor_pos())
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum::<usize>()
            .min(query_budget)
            .min(u16::MAX as usize) as u16;
        let max_x = area
            .x
            .saturating_add(prefix_width as u16)
            .saturating_add(query_budget.min(u16::MAX as usize) as u16);
        let cursor_x = (area.x + prefix_width as u16)
            .saturating_add(cursor_offset)
            .min(max_x)
            .min(area.x + area.width.saturating_sub(1));
        frame.set_cursor_position(Position::new(cursor_x, area.y));
    }
}

fn centered_modal_area(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    let width = preferred_width.min(area.width);
    let height = preferred_height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn render_confirm_dialog(frame: &mut Frame, area: Rect) {
    let prompt = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "Delete this conversation? ",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled("(y/n)", Style::default().fg(rgb(th().text_secondary))),
    ]);
    let paragraph = Paragraph::new(prompt);
    frame.render_widget(paragraph, area);
}

fn render_rename_dialog(frame: &mut Frame, input: &str, cursor: usize) {
    let area = frame.area();
    let menu_width = area.width.saturating_sub(4).clamp(30, 70);
    let menu_height = 4;
    let menu_area = Rect {
        x: (area.width.saturating_sub(menu_width)) / 2,
        y: (area.height.saturating_sub(menu_height)) / 2,
        width: menu_width,
        height: menu_height,
    };

    frame.render_widget(Clear, menu_area);
    let background = Block::default().style(Style::default().bg(rgb(th().overlay_bg)));
    frame.render_widget(background, menu_area);

    let block = Block::default()
        .title(" Rename session ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(th().accent)));
    let inner = block.inner(menu_area);
    frame.render_widget(block, menu_area);

    let input_width = inner.width.saturating_sub(2) as usize;
    let display = simple_truncate(input, input_width);
    let lines = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled(display, Style::default().fg(rgb(th().text_primary))),
        ]),
        Line::styled(
            " Enter save · Esc cancel",
            Style::default().fg(rgb(th().text_muted)),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inner);

    let cursor_offset: u16 = input
        .chars()
        .take(cursor)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum::<usize>()
        .min(input_width) as u16;
    frame.set_cursor_position(Position::new(
        inner.x.saturating_add(1).saturating_add(cursor_offset),
        inner.y,
    ));
}

fn render_export_menu(frame: &mut Frame, selected: usize, is_yank: bool) {
    let title = if is_yank {
        "Copy to clipboard"
    } else {
        "Export to file"
    };
    let options = [
        "[1] Ledger (formatted)",
        "[2] Plain text",
        "[3] Markdown",
        "[4] JSONL (raw)",
    ];

    let area = frame.area();
    let menu_width = 35;
    let menu_height = options.len() as u16 + 4; // options + title + border + cancel hint

    let menu_area = centered_modal_area(area, menu_width, menu_height);

    // Clear the area behind the modal first
    frame.render_widget(Clear, menu_area);

    // Render background
    let background = Block::default().style(Style::default().bg(rgb(th().overlay_bg)));
    frame.render_widget(background, menu_area);

    // Render border
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(th().accent)));

    let inner = block.inner(menu_area);
    frame.render_widget(block, menu_area);

    // Render options
    let mut lines = Vec::new();
    for (i, opt) in options.iter().enumerate() {
        let style = if i == selected {
            Style::default().fg(rgb(th().accent)).bold()
        } else {
            Style::default().fg(rgb(th().text_primary))
        };
        let prefix = if i == selected { "▶ " } else { "  " };
        lines.push(Line::styled(format!("{}{}", prefix, opt), style));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "  [Esc] Cancel",
        Style::default().fg(rgb(th().text_muted)),
    ));

    if inner.is_empty() {
        return;
    }

    let menu_content = Paragraph::new(lines);
    frame.render_widget(menu_content, inner);
}

fn render_help_overlay(
    frame: &mut Frame,
    is_view_mode: bool,
    is_single_file_mode: bool,
    semantic_available: bool,
    keys: &KeyBindings,
    scroll: usize,
) {
    let exit_text = if is_single_file_mode {
        "Quit"
    } else {
        "Back to list"
    };

    let shortcuts: Vec<(String, &str)> = if is_view_mode {
        vec![
            ("j / ↓".into(), "Scroll down"),
            ("k / ↑".into(), "Scroll up"),
            ("J / ]".into(), "Next message"),
            ("K / [".into(), "Previous message"),
            ("} / {".into(), "Next / prev prompt"),
            ("d / Ctrl+D".into(), "Half page down"),
            ("u / Ctrl+U".into(), "Half page up"),
            ("g / Home".into(), "Jump to top"),
            ("G / End".into(), "Jump to bottom"),
            ("/".into(), "Search"),
            ("n / N".into(), "Next / prev match"),
            ("t".into(), "Cycle tools: summary/short/full"),
            ("T".into(), "Toggle thinking"),
            ("i".into(), "Toggle timing"),
            ("e".into(), "Export to file"),
            ("y".into(), "Copy to clipboard / message"),
            ("p".into(), "Show file path"),
            ("Y".into(), "Copy path"),
            ("I".into(), "Copy session ID"),
            (keys.resume.help_label(), "Resume"),
            (keys.fork.help_label(), "Fork resume"),
            (keys.delete.help_label(), "Delete"),
            ("q / Esc".into(), exit_text),
        ]
    } else {
        let mut shortcuts = vec![
            ("↑ / ↓".into(), "Move selection"),
            ("← / →".into(), "Move cursor"),
            ("Ctrl+P / N".into(), "Move selection"),
            ("Ctrl+D".into(), "Half page down"),
            ("Ctrl+U".into(), "Kill to start of line"),
            ("Ctrl+K".into(), "Kill to end of line"),
            ("PgUp / PgDn".into(), "Jump by page"),
            ("Home / End".into(), "Jump to first/last"),
            ("Tab".into(), "Toggle scope (All/Project)"),
            ("Shift+Tab".into(), "Cycle source (when several)"),
            (keys.sort.help_label(), "Sort: best match / newest"),
            ("Enter".into(), "Open viewer"),
            ("Ctrl+O".into(), "Select and exit"),
            ("Ctrl+W".into(), "Delete word"),
            (keys.resume.help_label(), "Resume"),
            (keys.fork.help_label(), "Fork resume"),
            (keys.rename.help_label(), "Rename"),
            (keys.delete.help_label(), "Delete"),
            ("Esc".into(), "Quit"),
        ];
        if semantic_available {
            shortcuts.insert(11, ("Ctrl+T".into(), "Toggle semantic search"));
            shortcuts.insert(12, ("Ctrl+S".into(), "Semantic details"));
        }
        shortcuts
    };

    let title = " Shortcuts ";

    let area = frame.area();
    // Calculate dimensions based on content (use chars().count() for Unicode)
    let max_key_len = shortcuts
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let max_action_len = shortcuts
        .iter()
        .map(|(_, a)| a.chars().count())
        .max()
        .unwrap_or(0);
    // Padding: 2 chars left + key + " │ " (3) + action + 2 chars right
    let menu_width = (max_key_len + max_action_len + 11) as u16;
    // Height: 1 top padding + shortcuts + 1 bottom padding + 2 border
    let menu_height = shortcuts.len() as u16 + 4;

    let menu_area = centered_modal_area(area, menu_width, menu_height);

    // Clear the area behind the modal
    frame.render_widget(Clear, menu_area);

    // Render background
    let background = Block::default().style(Style::default().bg(rgb(th().overlay_bg)));
    frame.render_widget(background, menu_area);

    // Render border
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(th().accent)));

    let inner = block.inner(menu_area);
    frame.render_widget(block, menu_area);

    if inner.is_empty() {
        return;
    }

    let content_height = inner.height as usize;
    let indicator_needed = shortcuts.len() > content_height;
    let shortcut_rows = if indicator_needed {
        content_height.saturating_sub(1)
    } else {
        content_height
    };
    let max_scroll = shortcuts.len().saturating_sub(shortcut_rows);
    let scroll = scroll.min(max_scroll);

    let mut lines = Vec::new();
    if !indicator_needed {
        lines.extend(
            (0..content_height.saturating_sub(shortcuts.len()) / 2).map(|_| Line::from("")),
        );
    }
    for (key, action) in shortcuts.iter().skip(scroll).take(shortcut_rows) {
        let key_padding = max_key_len - key.chars().count();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{}{}", key, " ".repeat(key_padding)),
                Style::default().fg(rgb(th().accent)),
            ),
            Span::styled(" │ ", Style::default().fg(rgb(th().border))),
            Span::styled(
                action.to_string(),
                Style::default().fg(rgb(th().text_primary)),
            ),
        ]));
    }

    if indicator_needed && content_height > 0 {
        let start = scroll + 1;
        let end = (scroll + shortcut_rows).min(shortcuts.len());
        let indicator = match (scroll > 0, scroll < max_scroll) {
            (true, true) => format!("  ↑↓ more  {start}-{end}/{}", shortcuts.len()),
            (true, false) => format!("  ↑ more  {start}-{end}/{}", shortcuts.len()),
            (false, true) => format!("  ↓ more  {start}-{end}/{}", shortcuts.len()),
            (false, false) => format!("  {start}-{end}/{}", shortcuts.len()),
        };
        lines.push(Line::styled(
            indicator,
            Style::default().fg(rgb(th().text_muted)),
        ));
    }

    let content = Paragraph::new(lines);
    frame.render_widget(content, inner);
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let matcher = QueryMatcher::from_query(app.query());

    let semantic_mode = app.list_search_mode() == ListSearchMode::Semantic;
    let lines_per_item = list_lines_per_item(app.list_search_mode(), app.query());
    let rows_per_page = list_layout::rows_per_page(area.height, lines_per_item);
    let offset = list_layout::scroll_offset(
        app.list_scroll(),
        app.selected(),
        rows_per_page,
        app.filtered().len(),
    );
    let visible_count = rows_per_page.max(1);

    // Compute now once for consistent relative timestamps across all visible items
    let now = Local::now();

    let visible_items: Vec<ListItem> = app
        .filtered()
        .iter()
        .skip(offset)
        .take(visible_count)
        .enumerate()
        .map(|(relative_idx, &conv_idx)| {
            let list_idx = offset + relative_idx;
            let is_selected = app.selected() == Some(list_idx);
            let source = RowSource {
                conversation: &app.conversations()[conv_idx],
                matcher: &matcher,
                semantic_mode,
                semantic: app.semantic_result_metadata(conv_idx),
                width,
                multiple_sources: app.has_multiple_sources(),
            };
            // The frame loop prepares evidence for every visible row; direct
            // callers (tests) get it computed inline.
            let evidence = match app.row_evidence(conv_idx) {
                Some(evidence) => Cow::Borrowed(evidence),
                None => Cow::Owned(row_evidence(&source)),
            };
            let row = project_row(&source, &evidence, now);
            let lines = row_lines(&row, &matcher, is_selected, lines_per_item);
            ListItem::new(lines)
        })
        .collect();

    let list = List::new(visible_items);
    frame.render_widget(list, area);
}

/// Style one projected row into `lines_per_item` lines: header, preview and
/// optional context (or a blank line to keep row height uniform, so
/// click-to-row math matches what is drawn). Rows are told apart by the
/// bright header over the muted preview, not by a rule line, so a page holds
/// half again as many rows.
/// Split `text` (the project part of a row, after any source badge) into a
/// leading ` · ` joiner, the repo name and a `/worktree` suffix, each with
/// its own base style from `styles`; match `ranges` over the whole text keep
/// highlighting across the split.
fn project_spans(
    text: &str,
    ranges: Vec<(usize, usize)>,
    styles: [Style; 3],
    match_style: Style,
) -> Vec<Span<'static>> {
    let name_start = text
        .strip_prefix(" \u{b7} ")
        .map_or(0, |_| " \u{b7} ".len());
    let suffix_start = text[name_start..]
        .find('/')
        .map_or(text.len(), |slash| name_start + slash);
    let bounds = [0, name_start, suffix_start, text.len()];
    let mut spans = Vec::new();
    for (segment, style) in styles.into_iter().enumerate() {
        let (start, end) = (bounds[segment], bounds[segment + 1]);
        if start == end {
            continue;
        }
        let clipped = ranges
            .iter()
            .filter_map(|&(from, to)| {
                let (from, to) = (from.max(start), to.min(end));
                (from < to).then(|| (from - start, to - start))
            })
            .collect();
        spans.extend(highlight_ranges(
            &text[start..end],
            clipped,
            style,
            match_style,
        ));
    }
    spans
}

fn row_lines(
    row: &ListRow,
    matcher: &QueryMatcher,
    is_selected: bool,
    lines_per_item: usize,
) -> Vec<Line<'static>> {
    let indicator_style = if is_selected {
        Style::default().fg(rgb(th().accent))
    } else {
        Style::default().fg(rgb(th().border))
    };
    let hue = rgb(th().project_color(&row.hue_key));
    let project_style = if is_selected {
        Style::default().fg(hue).bold()
    } else {
        Style::default().fg(hue)
    };
    let project_match_style = project_style.bold().underlined();
    let suffix_style = Style::default().fg(rgb(th().project_suffix));
    let highlight_style = if is_selected {
        Style::default().fg(rgb(th().accent)).bold()
    } else {
        Style::default().fg(rgb(th().accent))
    };
    let selection_bg = if is_selected {
        Style::default().bg(rgb(th().selection_bg))
    } else {
        Style::default()
    };
    let dot = || Span::styled(" · ", Style::default().fg(rgb(th().dot_separator)));

    let mut header_spans = vec![Span::styled(INDICATOR, indicator_style)];
    let (badge, project) = row.project.split_at(row.badge_len);
    if !badge.is_empty() {
        header_spans.push(Span::styled(
            badge.to_string(),
            Style::default().fg(rgb(th().accent_dim)),
        ));
    }
    header_spans.extend(project_spans(
        project,
        matcher.ranges(project),
        [suffix_style, project_style, suffix_style],
        project_match_style,
    ));
    if let Some(title) = &row.custom_title {
        header_spans.extend(highlight(
            matcher,
            title,
            Style::default().fg(rgb(th().custom_title)),
            Style::default().fg(rgb(th().custom_title_highlight)),
        ));
    }
    if let Some(summary) = &row.summary {
        header_spans.extend(highlight(
            matcher,
            summary,
            Style::default().fg(rgb(th().summary)),
            Style::default().fg(rgb(th().summary_highlight)),
        ));
    }
    header_spans.push(Span::raw(" ".repeat(row.padding)));
    header_spans.push(Span::styled(
        row.msg_count.clone(),
        Style::default().fg(rgb(th().msg_count)),
    ));
    if let Some(meta) = &row.semantic_meta {
        header_spans.push(dot());
        header_spans.push(Span::styled(
            meta.clone(),
            Style::default().fg(rgb(th().accent)),
        ));
    }
    if let Some(duration) = &row.duration {
        header_spans.push(dot());
        header_spans.push(Span::styled(
            duration.clone(),
            Style::default().fg(rgb(th().duration_color)),
        ));
    }
    header_spans.push(dot());
    let timestamp_color = match row.recency {
        Recency::Now => th().timestamp_now,
        Recency::Minutes => th().timestamp_minutes,
        Recency::Hours => th().timestamp_hours,
        Recency::Days => th().timestamp_days,
        Recency::Old => th().text_secondary,
    };
    header_spans.push(Span::styled(
        row.timestamp.clone(),
        Style::default().fg(rgb(timestamp_color)),
    ));
    let header = Line::from(header_spans).style(selection_bg);

    let mut preview_spans = vec![Span::styled(INDICATOR, indicator_style)];
    preview_spans.extend(highlight(
        matcher,
        &row.preview,
        Style::default().fg(rgb(th().preview)),
        highlight_style,
    ));
    let preview = Line::from(preview_spans).style(selection_bg);

    let context = row.context.as_ref().map(|context_text| {
        let mut context_spans = vec![Span::styled(INDICATOR, indicator_style)];
        context_spans.extend(highlight(
            matcher,
            context_text,
            Style::default().fg(rgb(th().context_base)),
            Style::default().fg(rgb(th().context_highlight)),
        ));
        Line::from(context_spans).style(selection_bg)
    });

    if let Some(ctx) = context {
        vec![header, preview, ctx]
    } else if lines_per_item == 3 {
        vec![header, preview, Line::default()]
    } else {
        vec![header, preview]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Conversation;
    use crate::semantic::types::{
        SemanticChunkIdentity, SemanticExplanation, SemanticQuality, SemanticRationaleKind,
        SemanticScoreBreakdown,
    };
    use crate::tui::app::{SemanticProgress, SemanticResultMetadata, TuiSearchOptions};
    use crate::tui::semantic_worker::{SemanticSearchMessage, SemanticSearchResponse};
    use crate::tui::viewer::ToolDisplayMode;
    use chrono::TimeZone;
    use ratatui::Terminal;
    use ratatui::backend::{Backend, TestBackend};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::mpsc;

    #[test]
    fn view_help_overlay_handles_tiny_terminal() {
        for (width, height) in [(20, 8), (10, 3), (2, 2), (1, 1)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_help_overlay(frame, true, false, false, &KeyBindings::default(), 0)
                })
                .unwrap();
        }
    }

    #[test]
    fn list_help_overlay_handles_tiny_terminal() {
        for (width, height) in [(20, 8), (10, 3), (2, 2), (1, 1)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_help_overlay(frame, false, false, false, &KeyBindings::default(), 0)
                })
                .unwrap();
        }
    }

    fn terminal_contents(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn row_text(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn assert_cursor_inside(terminal: &mut Terminal<TestBackend>, width: u16) {
        let cursor = terminal.backend_mut().get_cursor_position().unwrap();
        assert_eq!(cursor.y, 0);
        assert!(cursor.x < width, "cursor {cursor:?} outside width {width}");
    }

    fn test_conversation() -> Conversation {
        Conversation {
            origin: None,
            source: crate::history::Source::Claude,
            session_id: "session".to_owned(),
            path: PathBuf::from("/tmp/session.jsonl"),
            index: 0,
            timestamp: Local.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            preview: "lexical preview sentinel".to_string(),
            preview_first: "lexical preview sentinel".to_string(),
            preview_last: "lexical preview sentinel".to_string(),
            full_text: "tool output sentinel summary sentinel cwd sentinel".to_string(),
            agent_search_text: String::new(),
            semantic_route_text: String::new(),
            semantic_turns: vec!["semantic visible text".to_string()],
            semantic_turn_ranges: vec![crate::history::MessageRange::single(1)],
            search_text_lower: "lexical preview sentinel".to_string(),
            dialogue_text_lower: String::new(),
            project_name: Some("project sentinel".to_string()),
            project_path: None,
            cwd: Some(PathBuf::from("/cwd/sentinel")),
            message_count: 1,
            parse_errors: Vec::new(),
            summary: Some("summary sentinel".to_string()),
            custom_title: Some("title sentinel".to_string()),
            model: None,
            total_tokens: 0,
            duration_minutes: None,
        }
    }

    fn semantic_app() -> App {
        App::new_with_options(
            vec![test_conversation()],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
            TuiSearchOptions {
                default_mode: ListSearchMode::Semantic,
                ..Default::default()
            },
        )
    }

    fn app_with_project_name(project_name: &str) -> App {
        let mut conversation = test_conversation();
        conversation.project_name = Some(project_name.to_string());
        conversation.custom_title = Some("semantic status title".to_string());
        conversation.summary = Some("semantic status summary".to_string());
        App::new(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        )
    }

    fn semantic_searching_app(query: &str, progress: SemanticProgress) -> App {
        let mut app = semantic_app();
        let (response_tx, response_rx) = mpsc::channel();
        app.set_query_for_test(query);
        app.set_semantic_receiver_for_test(7, response_rx);
        app.set_semantic_prewarm_generation_for_test(7);
        response_tx
            .send(SemanticSearchMessage::Progress {
                generation: 7,
                progress,
            })
            .unwrap();
        app.receive_search_results();
        app
    }

    fn test_semantic_metadata(evidence_preview: &str) -> SemanticResultMetadata {
        test_semantic_metadata_with_scores(
            evidence_preview,
            SemanticScoreBreakdown {
                hybrid: 1.0,
                semantic: 1.0,
                lexical: 0.0,
            },
            SemanticRationaleKind::SemanticOnly,
        )
    }

    fn test_semantic_metadata_with_scores(
        evidence_preview: &str,
        score_breakdown: SemanticScoreBreakdown,
        rationale_kind: SemanticRationaleKind,
    ) -> SemanticResultMetadata {
        SemanticResultMetadata {
            score_breakdown,
            explanation: SemanticExplanation {
                quality: SemanticQuality::Strong,
                quality_label: "strong",
                matched_terms: Vec::new(),
                evidence_preview: evidence_preview.to_string(),
                rationale_kind,
                chunk: SemanticChunkIdentity {
                    conversation_index: 0,
                    source: crate::semantic::types::SemanticChunkSource::VisibleDialogue,
                    session: "test-session".to_string(),
                    chunk_index: 0,
                    message_range: crate::history::MessageRange::single(1),
                },
            },
        }
    }

    #[test]
    fn search_bar_hides_transient_semantic_status_at_narrow_width() {
        let app = semantic_searching_app("你好世界widequery", SemanticProgress::Ranking);
        let width = 24;
        let backend = TestBackend::new(width, 4);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_search_bar(frame, &app, frame.area()))
            .unwrap();

        let line = row_text(&terminal, 0);
        assert_eq!(line.chars().count(), width as usize);
        assert!(!line.contains("sem ranking"), "{line:?}");
        assert!(!line.contains("sem model"), "{line:?}");
        assert!(!line.contains("sem cache"), "{line:?}");
        assert!(line.contains("1/1"), "{line:?}");
        assert_cursor_inside(&mut terminal, width);
    }

    #[test]
    fn lexical_search_bar_omits_semantic_status_at_normal_width() {
        let mut app = App::new(
            vec![test_conversation()],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        app.set_query_for_test("lexical query");
        let width = 80;
        let backend = TestBackend::new(width, 4);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_search_bar(frame, &app, frame.area()))
            .unwrap();

        let line = row_text(&terminal, 0);
        assert_eq!(line.chars().count(), width as usize);
        assert!(line.contains("lexical query"), "{line:?}");
        assert!(line.contains("1/1"), "{line:?}");
        assert!(!line.contains("semantic"), "{line:?}");
        assert!(!line.contains("sem "), "{line:?}");
        assert!(!line.contains("lex "), "{line:?}");
        assert_cursor_inside(&mut terminal, width);
    }

    #[test]
    fn semantic_search_bar_keeps_query_mode_count_status_and_cursor_at_normal_width() {
        let app = semantic_searching_app(
            "vector query with enough words",
            SemanticProgress::Embedding {
                completed: 21,
                total: 42,
            },
        );
        let width = 80;
        let backend = TestBackend::new(width, 4);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_search_bar(frame, &app, frame.area()))
            .unwrap();

        let line = row_text(&terminal, 0);
        assert_eq!(line.chars().count(), width as usize);
        assert!(line.contains("vector query with enough words"), "{line:?}");
        assert!(line.contains("sem 1/1"), "{line:?}");
        assert!(line.contains("1/1"), "{line:?}");
        assert!(!line.contains("sem embedding"), "{line:?}");
        assert_cursor_inside(&mut terminal, width);
    }

    fn complete_semantic_search(app: &mut App, metadata: SemanticResultMetadata) {
        let (response_tx, response_rx) = mpsc::channel();
        app.set_semantic_receiver_for_test(7, response_rx);
        response_tx
            .send(SemanticSearchMessage::Complete(SemanticSearchResponse {
                generation: 7,
                filtered: vec![0],
                metadata: HashMap::from([(0, metadata)]),
                error: None,
                progress: SemanticProgress::Complete,
                prewarm: false,
            }))
            .unwrap();
        app.receive_search_results();
    }

    fn render_semantic_list_contents(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        terminal_contents(&terminal)
    }

    #[test]
    fn list_truncates_long_project_names_on_narrow_rows() {
        let app = app_with_project_name("claude-history/drop-semantic-feature-gate");
        let backend = TestBackend::new(70, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let first_row = row_text(&terminal, 0);
        assert!(
            first_row.contains("claude-history/drop-semantic…"),
            "{first_row:?}"
        );
        assert!(
            !first_row.contains("claude-history/drop-semantic-feature-gate"),
            "{first_row:?}"
        );
    }

    #[test]
    fn list_uses_available_width_for_custom_titles() {
        let mut conversation = test_conversation();
        conversation.project_name = Some("aven".to_string());
        conversation.custom_title = Some(
            "fork lineage alpha beta gamma delta epsilon zeta eta theta iota kappa lambda"
                .to_string(),
        );
        conversation.summary = Some("generated summary remains visible".to_string());
        let app = App::new(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        let backend = TestBackend::new(160, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let first_row = row_text(&terminal, 0);
        assert!(
            first_row.contains(
                "fork lineage alpha beta gamma delta epsilon zeta eta theta iota kappa lambda"
            ),
            "{first_row:?}"
        );
        assert!(
            first_row.contains("generated summary remains visible"),
            "{first_row:?}"
        );
        assert!(first_row.contains("1 msg · Jan 01, 00:00"), "{first_row:?}");
    }

    #[test]
    fn list_truncates_custom_titles_to_preserve_metadata() {
        let mut conversation = test_conversation();
        conversation.project_name = Some("aven".to_string());
        conversation.custom_title = Some(
            "fork lineage alpha beta gamma delta epsilon zeta eta theta iota kappa lambda"
                .to_string(),
        );
        conversation.summary = None;
        let app = App::new(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        let backend = TestBackend::new(72, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let first_row = row_text(&terminal, 0);
        assert!(
            first_row.contains("fork lineage alpha beta gamma delta e…"),
            "{first_row:?}"
        );
        assert!(first_row.contains("1 msg · Jan 01, 00:00"), "{first_row:?}");
        assert_eq!(
            UnicodeWidthStr::width(first_row.as_str()),
            72,
            "{first_row:?}"
        );
    }

    #[test]
    fn semantic_list_uses_conversation_preview_without_query() {
        let app = semantic_app();
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(
            contents.contains("lexical preview sentinel"),
            "{contents:?}"
        );
        assert!(!contents.contains("semantic visible text"), "{contents:?}");
    }

    #[test]
    fn semantic_list_shows_lexical_context_while_query_has_no_metadata() {
        // Until the semantic worker answers, rows fall back to the lexical
        // ranking and show its hidden-context snippet like lexical mode does.
        let mut app = semantic_app();
        app.set_query_for_test("sentinel");
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("tool output sentinel"), "{contents:?}");
    }

    #[test]
    fn semantic_list_shows_compact_score_metadata_on_wide_rows() {
        let mut app = semantic_app();
        app.set_query_for_test("sentinel");
        complete_semantic_search(
            &mut app,
            test_semantic_metadata_with_scores(
                "semantic evidence only",
                SemanticScoreBreakdown {
                    hybrid: 1.23,
                    semantic: 1.0,
                    lexical: 0.23,
                },
                SemanticRationaleKind::LexicalBoosted,
            ),
        );
        let backend = TestBackend::new(70, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("1.23"), "{contents:?}");
        assert!(!contents.contains("strong"), "{contents:?}");
        assert!(!contents.contains("good"), "{contents:?}");
    }

    #[test]
    fn semantic_list_hides_score_metadata_on_narrow_rows() {
        let mut app = semantic_app();
        app.set_query_for_test("sentinel");
        complete_semantic_search(
            &mut app,
            test_semantic_metadata_with_scores(
                "semantic evidence only",
                SemanticScoreBreakdown {
                    hybrid: 1.23,
                    semantic: 1.0,
                    lexical: 0.23,
                },
                SemanticRationaleKind::LexicalBoosted,
            ),
        );
        let backend = TestBackend::new(69, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(!contents.contains("1.23"), "{contents:?}");
        assert!(!contents.contains("strong"), "{contents:?}");
        assert!(!contents.contains("good"), "{contents:?}");
    }

    #[test]
    fn narrow_status_bar_drops_low_priority_hints_whole() {
        let hint = |priority, text: &str| Hint {
            priority,
            right: false,
            spans: vec![Span::raw(text.to_string())],
        };
        let hints = vec![
            hint(1, "open"),
            hint(7, "rename"),
            hint(0, "help"),
            hint(5, "fork"),
        ];
        let kept = |width| {
            fit_hints(
                vec![
                    hint(1, "open"),
                    hint(7, "rename"),
                    hint(0, "help"),
                    hint(5, "fork"),
                ],
                width,
            )
            .iter()
            .map(|hint| hint.spans[0].content.to_string())
            .collect::<Vec<_>>()
        };
        assert_eq!(fit_hints(hints, 100).len(), 4);
        assert_eq!(kept(18), vec!["open", "help", "fork"]);
        assert_eq!(kept(12), vec!["open", "help"]);
        assert_eq!(kept(3), Vec::<String>::new());
    }

    #[test]
    fn list_status_bar_never_cuts_a_hint_at_80_columns() {
        let app = App::new(
            vec![test_conversation()],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_list_status_bar(frame, &app, frame.area()))
            .unwrap();
        let line = row_text(&terminal, 0);
        assert!(line.contains("Enter open"), "{line:?}");
        assert!(line.trim_end().ends_with("? help"), "{line:?}");
    }

    #[test]
    fn view_header_measures_display_width_not_bytes() {
        let mut conv = test_conversation();
        conv.custom_title = None;
        conv.project_name = Some("proj".to_string());
        // 20 columns, 40 bytes.
        conv.summary = Some("é".repeat(20));
        let path = conv.path.clone();
        let metadata = ViewHeader::new(Some(&conv), &path, 500);
        let inline_width = header_width(&metadata.parts) + separated(&"é".repeat(20));

        let fits = ViewHeader::new(Some(&conv), &path, inline_width as u16);
        assert!(fits.summary_inline, "{fits:?}");
        assert_eq!(fits.height(), 2);

        let narrower = ViewHeader::new(Some(&conv), &path, inline_width as u16 - 1);
        assert!(!narrower.summary_inline);
        assert_eq!(narrower.height(), 3);
    }

    #[test]
    fn viewer_status_bar_offers_resume_while_a_search_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut conversation = test_conversation();
        conversation.path = dir.path().join("session.jsonl");
        let user = serde_json::json!({
            "type": "user",
            "timestamp": "2024-01-01T00:00:00Z",
            "message": {"role": "user", "content": "the warming fix"}
        });
        std::fs::write(&conversation.path, format!("{user}\n")).unwrap();
        let mut app = App::new(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        app.set_query_for_test("warming");
        app.enter_view_mode(80);
        let AppMode::View(state) = app.app_mode() else {
            unreachable!()
        };
        assert!(!state.search_matches.is_empty());

        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_view_status_bar(frame, &app, state, frame.area()))
            .unwrap();
        let line = row_text(&terminal, 0);
        assert!(line.contains("match 1/1"), "{line:?}");
        assert!(line.contains("^R resume"), "{line:?}");
    }

    #[test]
    fn semantic_status_bar_keeps_hotkeys_when_result_metadata_exists() {
        let mut app = App::new_with_options(
            vec![test_conversation(), test_conversation()],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
            TuiSearchOptions {
                default_mode: ListSearchMode::Semantic,
                ..Default::default()
            },
        );
        app.set_query_for_test("sentinel");
        let (response_tx, response_rx) = mpsc::channel();
        app.set_semantic_receiver_for_test(7, response_rx);
        response_tx
            .send(SemanticSearchMessage::Complete(SemanticSearchResponse {
                generation: 7,
                filtered: vec![1, 0],
                metadata: HashMap::from([(
                    1,
                    test_semantic_metadata_with_scores(
                        "semantic evidence only",
                        SemanticScoreBreakdown {
                            hybrid: 1.23,
                            semantic: 0.98,
                            lexical: 0.25,
                        },
                        SemanticRationaleKind::LexicalBoosted,
                    ),
                )]),
                error: None,
                progress: SemanticProgress::Complete,
                prewarm: false,
            }))
            .unwrap();
        app.receive_search_results();
        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list_status_bar(frame, &app, frame.area()))
            .unwrap();

        let line = row_text(&terminal, 0);
        assert!(line.contains("Enter"), "{line:?}");
        assert!(line.contains("search·sem"), "{line:?}");
        assert!(!line.contains("sem 0.98"), "{line:?}");
        assert!(!line.contains("lex 0.25"), "{line:?}");
        assert!(!line.contains("lex boost"), "{line:?}");
    }

    #[test]
    fn semantic_status_bar_shows_embedding_progress_before_results() {
        let app = semantic_searching_app(
            "sentinel",
            SemanticProgress::Embedding {
                completed: 21,
                total: 42,
            },
        );
        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list_status_bar(frame, &app, frame.area()))
            .unwrap();

        let line = row_text(&terminal, 0);
        assert!(line.contains("sem embedding 50%"), "{line:?}");
        assert!(line.contains("21/42 chunks"), "{line:?}");
    }

    #[test]
    fn semantic_debug_popup_renders_score_details() {
        let mut app = semantic_app();
        app.set_query_for_test("sentinel");
        complete_semantic_search(
            &mut app,
            test_semantic_metadata_with_scores(
                "semantic evidence only",
                SemanticScoreBreakdown {
                    hybrid: 1.23,
                    semantic: 0.9,
                    lexical: 0.1,
                },
                SemanticRationaleKind::LexicalBoosted,
            ),
        );
        app.set_dialog_mode_for_test(DialogMode::SemanticDebug);
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list_mode(frame, &app))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("Semantic result"), "{contents:?}");
        assert!(contents.contains("1.23"), "{contents:?}");
        assert!(contents.contains("0.90"), "{contents:?}");
        assert!(contents.contains("lex boost"), "{contents:?}");
        assert!(contents.contains("semantic evidence only"), "{contents:?}");
    }

    #[test]
    fn quoted_list_highlighting_matches_literal_text() {
        let highlight_style = Style::default().fg(Color::Yellow);
        let query = QueryMatcher::from_query("\"DEPLOYMENT_TOKEN\"");
        let spans = highlight(
            &query,
            "prefix DEPLOYMENT_TOKEN suffix",
            Style::default(),
            highlight_style,
        );
        let highlighted: Vec<_> = span_info(&spans, highlight_style)
            .into_iter()
            .filter(|(_, highlighted)| *highlighted)
            .collect();

        assert_eq!(highlighted, vec![("DEPLOYMENT_TOKEN", true)]);
    }

    #[test]
    fn quoted_list_highlighting_matches_multiword_literal_phrase() {
        let highlight_style = Style::default().fg(Color::Yellow);
        let query = QueryMatcher::from_query("alpha \"beta gamma\"");
        let spans = highlight(
            &query,
            "alpha prefix beta gamma suffix beta-only",
            Style::default(),
            highlight_style,
        );
        let highlighted: Vec<_> = span_info(&spans, highlight_style)
            .into_iter()
            .filter(|(_, highlighted)| *highlighted)
            .collect();

        assert_eq!(highlighted, vec![("alpha", true), ("beta gamma", true)]);
    }

    #[test]
    fn quoted_list_highlighting_respects_smart_case() {
        let highlight_style = Style::default().fg(Color::Yellow);
        let query = QueryMatcher::from_query("\"Beta Gamma\"");
        let spans = highlight(
            &query,
            "beta gamma then Beta Gamma",
            Style::default(),
            highlight_style,
        );
        let highlighted: Vec<_> = span_info(&spans, highlight_style)
            .into_iter()
            .filter(|(_, highlighted)| *highlighted)
            .collect();

        assert_eq!(highlighted, vec![("Beta Gamma", true)]);
    }

    #[test]
    fn semantic_evidence_preview_highlights_query_terms() {
        let metadata = test_semantic_metadata(
            "prefix text before the important semantic needle appears near the end",
        );
        let matcher = QueryMatcher::from_query("needle");
        let preview = &metadata.explanation.evidence_preview;
        let fitted = fit_around_matches(preview, &matcher.ranges(preview), 40);
        let spans = highlight(
            &matcher,
            &fitted,
            Style::default(),
            Style::default().fg(Color::Yellow),
        );
        let highlighted: Vec<_> = span_info(&spans, Style::default().fg(Color::Yellow))
            .into_iter()
            .filter(|(_, highlighted)| *highlighted)
            .collect();
        assert_eq!(highlighted.len(), 1);
        assert_eq!(highlighted[0].0, "needle");
    }

    #[test]
    fn semantic_list_truncates_cleanly_at_narrow_width() {
        let mut app = semantic_app();
        app.set_query_for_test("needle");
        let evidence_preview = format!("{} needle{}", "宽字符前缀".repeat(8), "x".repeat(120));
        complete_semantic_search(
            &mut app,
            test_semantic_metadata_with_scores(
                &evidence_preview,
                SemanticScoreBreakdown {
                    hybrid: 123.45,
                    semantic: 67.89,
                    lexical: 55.56,
                },
                SemanticRationaleKind::WeakMatch,
            ),
        );
        let width = 28;
        let height = 8;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list_mode(frame, &app))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("needle"), "{contents:?}");
        let truncated = fit_around_matches(
            &evidence_preview,
            &QueryMatcher::from_query("needle").ranges(&evidence_preview),
            width.saturating_sub(4) as usize,
        );
        assert!(truncated.contains("needle"), "{truncated:?}");
        assert!(
            UnicodeWidthStr::width(truncated.as_str()) <= width.saturating_sub(4) as usize,
            "{truncated:?}"
        );
        for y in 0..height {
            let line = row_text(&terminal, y);
            assert_eq!(line.chars().count(), width as usize, "{line:?}");
        }
    }

    #[test]
    fn lexical_unquoted_render_shows_hidden_full_text_context() {
        let mut conversation = test_conversation();
        conversation.preview = "visible lexical preview".to_string();
        conversation.full_text =
            format!("visible lexical preview {} hiddenneedle", "x ".repeat(200));
        let mut app = App::new(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        app.set_query_for_test("hiddenneedle");
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("hiddenneedle"), "{contents:?}");
    }

    #[test]
    fn lexical_quoted_render_shows_hidden_literal_context() {
        let mut conversation = test_conversation();
        conversation.preview = "visible lexical preview".to_string();
        conversation.full_text =
            format!("visible lexical preview {} hidden_literal", "x ".repeat(80));
        let mut app = App::new(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        app.set_query_for_test("\"hidden_literal\"");
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("hidden_literal"), "{contents:?}");
    }

    #[test]
    fn literal_query_rows_without_context_keep_the_three_line_pitch() {
        // Row 1 shows its literal in the preview (no context line); row 2
        // hides it in full_text (context line). Both must occupy three lines
        // so that click-to-row math stays aligned with what is drawn.
        let mut visible = test_conversation();
        visible.preview = "preview with hidden_literal shown".to_string();
        visible.full_text = visible.preview.clone();
        let mut hidden = test_conversation();
        hidden.preview = "visible lexical preview".to_string();
        hidden.full_text = format!("visible lexical preview {} hidden_literal", "x ".repeat(80));
        let mut app = App::new(
            vec![visible, hidden],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
        );
        app.set_query_for_test("\"hidden_literal\"");
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        // Headers carry the message count; each row starts three lines on.
        let header_rows: Vec<u16> = (0..12)
            .filter(|&y| row_text(&terminal, y).contains(" msg"))
            .collect();
        assert_eq!(
            header_rows,
            vec![0, 3],
            "{:?}",
            terminal_contents(&terminal)
        );
    }

    #[test]
    fn lexical_mixed_query_context_uses_unquoted_and_literals() {
        let mut conversation = test_conversation();
        conversation.preview = "visible preview".to_string();
        conversation.full_text = format!("hidden_unquoted {} exact_literal", "x ".repeat(120));
        let matcher = QueryMatcher::from_query("hidden_unquoted \"exact_literal\"");
        let source = RowSource {
            conversation: &conversation,
            matcher: &matcher,
            semantic_mode: false,
            semantic: None,
            width: 124,
            multiple_sources: false,
        };
        let evidence = row_evidence(&source);
        let row = project_row(&source, &evidence, Local::now());

        assert!(row.preview.contains("exact_literal"), "{:?}", row.preview);
        assert!(row.preview.contains("hidden_unquoted"), "{:?}", row.preview);
    }

    #[test]
    fn semantic_list_uses_semantic_evidence_preview_without_full_text_context() {
        let mut app = semantic_app();
        app.set_query_for_test("sentinel");
        complete_semantic_search(&mut app, test_semantic_metadata("semantic evidence only"));
        let contents = render_semantic_list_contents(&mut app, 80, 8);
        assert!(contents.contains("semantic evidence only"), "{contents:?}");
        assert!(
            !contents.contains("lexical preview sentinel"),
            "{contents:?}"
        );
        assert!(!contents.contains("tool output sentinel"), "{contents:?}");
    }

    #[test]
    fn semantic_list_shows_hidden_literal_for_rows_without_metadata() {
        let mut conversation = test_conversation();
        conversation.full_text =
            "tool output sentinel includes audio_generation literal".to_string();
        let mut app = App::new_with_options(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
            TuiSearchOptions {
                default_mode: ListSearchMode::Semantic,
                ..Default::default()
            },
        );
        app.set_query_for_test("semantic \"audio_generation\"");
        let (response_tx, response_rx) = mpsc::channel();
        app.set_semantic_receiver_for_test(7, response_rx);
        response_tx
            .send(SemanticSearchMessage::Complete(SemanticSearchResponse {
                generation: 7,
                filtered: vec![0],
                metadata: HashMap::new(),
                error: None,
                progress: SemanticProgress::Complete,
                prewarm: false,
            }))
            .unwrap();
        app.receive_search_results();
        let contents = render_semantic_list_contents(&mut app, 80, 8);
        assert!(contents.contains("audio_generation"), "{contents:?}");
    }

    #[test]
    fn semantic_literal_preview_uses_literal_ranges() {
        let mut app = semantic_app();
        app.set_query_for_test("\"audio_generation\"");
        complete_semantic_search(
            &mut app,
            test_semantic_metadata(
                "normalized audio generation appears early before exact audio_generation literal",
            ),
        );
        let backend = TestBackend::new(54, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("audio_generation"), "{contents:?}");
        assert!(!contents.contains("audio generation"), "{contents:?}");
    }

    #[test]
    fn semantic_literal_preview_merges_overlapping_ranges() {
        let mut app = semantic_app();
        app.set_query_for_test("audio \"audio_generation\"");
        complete_semantic_search(
            &mut app,
            test_semantic_metadata("prefix audio_generation literal near the front"),
        );
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("audio_generation"), "{contents:?}");
    }

    #[test]
    fn semantic_literal_context_requires_all_literals_visible() {
        let mut conversation = test_conversation();
        conversation.full_text = "alpha_exact near preview. beta_exact hidden deeper.".to_string();
        let mut app = App::new_with_options(
            vec![conversation],
            ToolDisplayMode::Truncated,
            false,
            KeyBindings::default(),
            vec![],
            TuiSearchOptions {
                default_mode: ListSearchMode::Semantic,
                ..Default::default()
            },
        );
        app.set_query_for_test("semantic \"alpha_exact\" \"beta_exact\"");
        complete_semantic_search(
            &mut app,
            test_semantic_metadata("semantic alpha_exact only"),
        );
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_list(frame, &app, frame.area()))
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("alpha_exact"), "{contents:?}");
        assert!(contents.contains("beta_exact"), "{contents:?}");
    }

    #[test]
    fn semantic_shortcut_appears_only_when_available() {
        let backend = TestBackend::new(70, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_help_overlay(frame, false, false, false, &KeyBindings::default(), 0)
            })
            .unwrap();
        let unavailable = terminal_contents(&terminal);

        let backend = TestBackend::new(70, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_help_overlay(frame, false, false, true, &KeyBindings::default(), 0)
            })
            .unwrap();
        let available = terminal_contents(&terminal);

        assert!(
            !unavailable.contains("Toggle semantic search"),
            "{unavailable:?}"
        );
        assert!(
            available.contains("Toggle semantic search"),
            "{available:?}"
        );
    }

    #[test]
    fn help_overlay_indicates_hidden_rows() {
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_help_overlay(frame, true, false, false, &KeyBindings::default(), 0)
            })
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(contents.contains("↓ more"), "{contents:?}");
        assert!(contents.contains("1-"), "{contents:?}");
    }

    #[test]
    fn help_overlay_scrolls_to_later_rows() {
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_help_overlay(frame, true, false, false, &KeyBindings::default(), 10)
            })
            .unwrap();

        let contents = terminal_contents(&terminal);
        assert!(
            contents.contains("↑↓ more") || contents.contains("↑ more"),
            "{contents:?}"
        );
        assert!(contents.contains("11-"), "{contents:?}");
    }

    #[test]
    fn export_menus_handle_tiny_terminal() {
        for is_yank in [false, true] {
            for (width, height) in [(20, 8), (10, 3), (2, 2), (1, 1)] {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal
                    .draw(|frame| render_export_menu(frame, 0, is_yank))
                    .unwrap();
            }
        }
    }

    #[test]
    fn centered_modal_area_preserves_fitting_size() {
        let area = centered_modal_area(Rect::new(0, 0, 80, 24), 35, 8);
        assert_eq!(area, Rect::new(22, 8, 35, 8));
    }

    #[test]
    fn centered_modal_area_clamps_to_frame() {
        assert_eq!(
            centered_modal_area(Rect::new(0, 0, 20, 24), 35, 8),
            Rect::new(0, 8, 20, 8)
        );
        assert_eq!(
            centered_modal_area(Rect::new(0, 0, 80, 3), 35, 8),
            Rect::new(22, 0, 35, 3)
        );
        assert_eq!(
            centered_modal_area(Rect::new(0, 0, 10, 3), 35, 8),
            Rect::new(0, 0, 10, 3)
        );
    }

    #[test]
    fn test_format_model_name_opus_45() {
        assert_eq!(format_model_name("claude-opus-4-5-20251101"), "opus-4.5");
    }

    #[test]
    fn test_format_model_name_sonnet_4() {
        assert_eq!(format_model_name("claude-sonnet-4-20250514"), "sonnet-4");
    }

    #[test]
    fn test_format_model_name_sonnet_35() {
        assert_eq!(
            format_model_name("claude-3-5-sonnet-20241022"),
            "sonnet-3.5"
        );
    }

    #[test]
    fn test_format_model_name_haiku_35() {
        assert_eq!(format_model_name("claude-3-5-haiku-20241022"), "haiku-3.5");
    }

    #[test]
    fn test_format_model_name_opus_3() {
        assert_eq!(format_model_name("claude-3-opus-20240229"), "opus-3");
    }

    #[test]
    fn test_format_model_name_unknown() {
        assert_eq!(format_model_name("custom-model"), "custom-model");
    }

    #[test]
    fn test_format_model_name_truncates_long() {
        let long_name = "very-long-unknown-model-name-that-exceeds-limit";
        let formatted = format_model_name(long_name);
        // 19 chars + ellipsis (3 bytes in UTF-8)
        assert!(formatted.chars().count() <= 20);
        assert!(formatted.ends_with('…'));
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1000), "1k");
        assert_eq!(format_tokens(417000), "417k");
        assert_eq!(format_tokens(999999), "999k");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(1_500_000), "1.5M");
        assert_eq!(format_tokens(12_345_678), "12.3M");
    }

    #[test]
    fn test_format_tokens_long() {
        assert_eq!(format_tokens_long(500), "500 tokens");
        assert_eq!(format_tokens_long(1000), "1k tokens");
        assert_eq!(format_tokens_long(926000), "926k tokens");
        assert_eq!(format_tokens_long(1_500_000), "1.5M tokens");
    }

    // --- span helpers ---

    /// Helper: extract (text, is_highlighted) from spans
    fn span_info<'a>(spans: &'a [Span<'a>], highlight_style: Style) -> Vec<(&'a str, bool)> {
        spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style == highlight_style))
            .collect()
    }
}
