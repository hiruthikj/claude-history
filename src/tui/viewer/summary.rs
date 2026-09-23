use crate::claude::{ContentBlock, LogEntry, UserContent};

use super::ledger::{LedgerRow, NameCol, push_row};
use super::style::assistant_label;
use super::timing::TimingSlot;
use super::tools::{
    ToolCallRenderSpec, ToolOutputKind, ToolResultRenderSpec, make_tool_output_id,
    render_tool_call, render_tool_result, tool_result_display_text,
};
use super::*;

pub(super) struct PendingToolSummary {
    pub(super) id: ToolOutputId,
    pub(super) first_entry_index: usize,
    pub(super) first_parsed_idx: usize,
    pub(super) last_parsed_idx: usize,
    pub(super) parent_id: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) timestamp: Option<String>,
    pub(super) summary: ToolActivitySummary,
}

#[derive(Default)]
pub(super) struct ToolActivitySummary {
    searched_patterns: usize,
    searched_file_patterns: usize,
    read_files: usize,
    shell_commands: usize,
    edited_files: usize,
    wrote_files: usize,
    agents: usize,
    fetched_urls: usize,
    web_searches: usize,
    /// Every other tool by display name, in first-call order.
    other_tools: Vec<(String, usize)>,
}

impl ToolActivitySummary {
    fn add_call(&mut self, name: &str) {
        match name {
            "Bash" => self.shell_commands += 1,
            "Read" => self.read_files += 1,
            "Grep" => self.searched_patterns += 1,
            "Glob" => self.searched_file_patterns += 1,
            "Edit" => self.edited_files += 1,
            "Write" => self.wrote_files += 1,
            "Task" | "Agent" => self.agents += 1,
            "WebFetch" => self.fetched_urls += 1,
            "WebSearch" => self.web_searches += 1,
            other => self.add_other(tool_display_name(other), 1),
        }
    }

    fn add_other(&mut self, name: &str, count: usize) {
        match self.other_tools.iter_mut().find(|(seen, _)| seen == name) {
            Some((_, seen_count)) => *seen_count += count,
            None => self.other_tools.push((name.to_string(), count)),
        }
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.searched_patterns += other.searched_patterns;
        self.searched_file_patterns += other.searched_file_patterns;
        self.read_files += other.read_files;
        self.shell_commands += other.shell_commands;
        self.edited_files += other.edited_files;
        self.wrote_files += other.wrote_files;
        self.agents += other.agents;
        self.fetched_urls += other.fetched_urls;
        self.web_searches += other.web_searches;
        for (name, count) in other.other_tools {
            self.add_other(&name, count);
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.searched_patterns
            + self.searched_file_patterns
            + self.read_files
            + self.shell_commands
            + self.edited_files
            + self.wrote_files
            + self.agents
            + self.fetched_urls
            + self.web_searches
            == 0
            && self.other_tools.is_empty()
    }

    fn sentence(&self) -> String {
        let mut parts = Vec::new();
        push_summary_item(
            &mut parts,
            self.searched_patterns,
            "Searched for",
            "pattern",
        );
        push_summary_item(
            &mut parts,
            self.searched_file_patterns,
            "Searched for",
            "file pattern",
        );
        push_summary_item(&mut parts, self.read_files, "read", "file");
        push_summary_item(&mut parts, self.shell_commands, "ran", "shell command");
        push_summary_item(&mut parts, self.edited_files, "edited", "file");
        push_summary_item(&mut parts, self.wrote_files, "wrote", "file");
        push_summary_item(&mut parts, self.agents, "started", "agent");
        push_summary_item(&mut parts, self.fetched_urls, "fetched", "URL");
        if self.web_searches > 0 {
            parts.push(match self.web_searches {
                1 => "searched the web".to_string(),
                n => format!("searched the web {n} times"),
            });
        }
        if !self.other_tools.is_empty() {
            parts.push(format!("called {}", other_tools_phrase(&self.other_tools)));
        }
        capitalize_first(parts.join(", "))
    }
}

/// How many named tools a summary lists before collapsing the rest.
const NAMED_OTHER_TOOLS: usize = 3;

/// `Skill`, `Skill ×2, ToolSearch`, `a, b, c +2 more`: names say more than
/// "called 3 tools", and the count keeps a long tail readable.
fn other_tools_phrase(tools: &[(String, usize)]) -> String {
    let mut named: Vec<String> = tools
        .iter()
        .take(NAMED_OTHER_TOOLS)
        .map(|(name, count)| match count {
            1 => name.clone(),
            n => format!("{name} ×{n}"),
        })
        .collect();
    let rest: usize = tools.iter().skip(NAMED_OTHER_TOOLS).map(|(_, n)| n).sum();
    if rest > 0 {
        named.push(format!("+{rest} more"));
    }
    named.join(", ")
}

/// MCP tools are `mcp__<server>__<tool>`; the tool part is what reads.
fn tool_display_name(name: &str) -> &str {
    match name.strip_prefix("mcp__") {
        Some(rest) => rest.rsplit_once("__").map_or(rest, |(_, tool)| tool),
        None => name,
    }
}

fn capitalize_first(text: String) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return text;
    };
    first.to_uppercase().chain(chars).collect()
}

fn push_summary_item(parts: &mut Vec<String>, count: usize, verb: &str, noun: &str) {
    if count == 0 {
        return;
    }
    let suffix = if count == 1 { "" } else { "s" };
    parts.push(format!("{verb} {count} {noun}{suffix}"));
}

pub(super) fn render_tool_activity_summary(
    lines: &mut Vec<RenderedLine>,
    label: &str,
    label_color: (u8, u8, u8),
    dimmed: bool,
    timing: TimingSlot<'_>,
    summary: &ToolActivitySummary,
    tool_output_id: Option<&ToolOutputId>,
) {
    if summary.is_empty() {
        return;
    }

    let content = vec![(
        summary.sentence(),
        LineStyle {
            fg: Some(th().tool_text),
            dimmed: true,
            ..Default::default()
        },
    )];
    push_row(
        lines,
        LedgerRow {
            timing,
            name: NameCol::Label {
                text: label,
                color: label_color,
                bold: false,
                dimmed,
            },
            separator_dimmed: dimmed,
            tool_output_id,
            clickable: tool_output_id.is_some(),
        },
        content,
    );
}

pub(super) fn summarize_tool_calls(blocks: &[ContentBlock]) -> ToolActivitySummary {
    let mut summary = ToolActivitySummary::default();
    for block in blocks {
        if let ContentBlock::ToolUse { name, .. } = block {
            summary.add_call(name);
        }
    }
    summary
}

fn assistant_blocks_are_tool_only(blocks: &[ContentBlock], show_thinking: bool) -> bool {
    blocks.iter().all(|block| {
        matches!(block, ContentBlock::ToolUse { .. })
            || (!show_thinking && matches!(block, ContentBlock::Thinking { .. }))
    })
}

pub(super) fn tool_only_assistant_summary<'a>(
    entry: &'a LogEntry,
    options: &RenderOptions,
) -> Option<(
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    ToolActivitySummary,
)> {
    let LogEntry::Assistant {
        message,
        agent,
        timestamp,
        parent_tool_use_id,
        ..
    } = entry
    else {
        return None;
    };

    if parent_tool_use_id.is_some() && !options.show_thinking {
        return None;
    }
    if message.content.is_empty()
        || !assistant_blocks_are_tool_only(&message.content, options.show_thinking)
    {
        return None;
    }

    let summary = summarize_tool_calls(&message.content);
    (!summary.is_empty()).then_some((
        parent_tool_use_id.as_deref(),
        agent.as_deref(),
        timestamp.as_deref(),
        summary,
    ))
}

pub(super) fn user_entry_is_only_tool_results(entry: &LogEntry, options: &RenderOptions) -> bool {
    let LogEntry::User {
        message,
        parent_tool_use_id,
        ..
    } = entry
    else {
        return false;
    };

    if parent_tool_use_id.is_some() && !options.show_thinking {
        return false;
    }

    let UserContent::Blocks(blocks) = &message.content else {
        return false;
    };
    !blocks.is_empty()
        && blocks
            .iter()
            .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn render_summary_group_details(
    lines: &mut Vec<RenderedLine>,
    entries: &[RenderableEntry],
    pending: &PendingToolSummary,
    options: &RenderOptions,
) {
    let first_line = lines.len();
    let mut rendered_any = false;
    let pad_timing = TimingSlot::from_show_timing(options.show_timing);
    let label = assistant_label(pending.parent_id.as_deref(), pending.agent.as_deref());
    for parsed in &entries[pending.first_parsed_idx..=pending.last_parsed_idx] {
        match &parsed.entry {
            LogEntry::Assistant {
                message,
                parent_tool_use_id,
                ..
            } if parent_tool_use_id.as_deref() == pending.parent_id.as_deref() => {
                for (block_idx, block) in message.content.iter().enumerate() {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        if rendered_any {
                            lines.push(RenderedLine::new(vec![]));
                        }
                        let output_id = make_tool_output_id(
                            parsed.entry_index,
                            parent_tool_use_id.as_deref(),
                            block_idx,
                            ToolOutputKind::ToolCall,
                            Some(id),
                        );
                        let expanded = options.expanded_tool_outputs.contains(&output_id);
                        render_tool_call(
                            lines,
                            &ToolCallRenderSpec {
                                name,
                                input,
                                label: &label,
                                label_color: th().accent_dim,
                                dimmed: true,
                                content_width: options.content_width,
                                timing: pad_timing,
                                tool_display: ToolDisplayMode::Truncated,
                                tool_output_id: &output_id,
                                expanded,
                            },
                        );
                        rendered_any = true;
                    }
                }
            }
            LogEntry::User {
                message,
                parent_tool_use_id,
                ..
            } if parent_tool_use_id.as_deref() == pending.parent_id.as_deref() => {
                let UserContent::Blocks(blocks) = &message.content else {
                    continue;
                };
                for (block_idx, block) in blocks.iter().enumerate() {
                    if let ContentBlock::ToolResult {
                        content,
                        tool_use_id,
                        ..
                    } = block
                    {
                        if rendered_any {
                            lines.push(RenderedLine::new(vec![]));
                        }
                        let output_id = make_tool_output_id(
                            parsed.entry_index,
                            parent_tool_use_id.as_deref(),
                            block_idx,
                            ToolOutputKind::ToolResult,
                            Some(tool_use_id),
                        );
                        let expanded = options.expanded_tool_outputs.contains(&output_id);
                        let content_str = tool_result_display_text(content.as_ref());
                        render_tool_result(
                            lines,
                            &ToolResultRenderSpec {
                                text: &content_str,
                                content_width: options.content_width,
                                timing: pad_timing,
                                tool_display: ToolDisplayMode::Truncated,
                                tool_output_id: &output_id,
                                expanded,
                            },
                        );
                        rendered_any = true;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(line) = lines.get_mut(first_line) {
        line.tool_output_id = Some(pending.id.clone());
        line.clickable = true;
    }
}

pub(super) fn flush_tool_summary(
    lines: &mut Vec<RenderedLine>,
    messages: &mut Vec<MessageRange>,
    pending: &mut Option<PendingToolSummary>,
    entries: &[RenderableEntry],
    options: &RenderOptions,
) {
    let Some(pending) = pending.take() else {
        return;
    };

    let start_line = lines.len();
    let label = assistant_label(pending.parent_id.as_deref(), pending.agent.as_deref());
    let ts = if options.show_timing {
        pending.timestamp.as_deref().and_then(format_timestamp)
    } else {
        None
    };
    let timing = match ts.as_deref() {
        Some(ts) => TimingSlot::Stamp(ts),
        None => TimingSlot::Disabled,
    };
    if options.expanded_tool_outputs.contains(&pending.id) {
        render_summary_group_details(lines, entries, &pending, options);
    } else {
        render_tool_activity_summary(
            lines,
            &label,
            th().accent_dim,
            pending.parent_id.is_some(),
            timing,
            &pending.summary,
            Some(&pending.id),
        );
    }

    let end_line = lines.len();
    if end_line > start_line {
        messages.push(MessageRange {
            entry_index: pending.first_entry_index,
            start_line,
            end_line,
            user_prompt: false,
        });
        lines.push(RenderedLine::new(vec![]));
    }
}
