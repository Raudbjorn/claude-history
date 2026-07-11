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
    pub(super) timestamp: Option<String>,
    pub(super) summary: ToolActivitySummary,
}

pub(super) struct ToolActivitySummaryInteraction<'a> {
    pub(super) tool_output_id: Option<&'a ToolOutputId>,
    pub(super) subagent_tool_use_id: Option<&'a str>,
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
    linked_agents: usize,
    tool_calls: usize,
    direct_subagent_id: Option<String>,
    /// Descriptions collected from all Agent/Task calls for richer display.
    agent_descriptions: Vec<String>,
    fetched_urls: usize,
    web_searches: usize,
    other_tools: usize,
}

impl ToolActivitySummary {
    fn add_call(
        &mut self,
        name: &str,
        id: &str,
        input: Option<&serde_json::Value>,
        linked_subagent: bool,
    ) {
        self.tool_calls += 1;
        self.direct_subagent_id = (self.tool_calls == 1 && linked_subagent).then(|| id.to_string());

        match name {
            "Bash" => self.shell_commands += 1,
            "Read" => self.read_files += 1,
            "Grep" => self.searched_patterns += 1,
            "Glob" => self.searched_file_patterns += 1,
            "Edit" => self.edited_files += 1,
            "Write" => self.wrote_files += 1,
            "Task" | "Agent" => {
                self.agents += 1;
                if linked_subagent {
                    self.linked_agents += 1;
                }
                if let Some(desc) = input.and_then(|value| value["description"].as_str()) {
                    self.agent_descriptions.push(desc.to_string());
                }
            }
            "WebFetch" => self.fetched_urls += 1,
            "WebSearch" => self.web_searches += 1,
            _ => self.other_tools += 1,
        }
    }

    pub(super) fn merge(&mut self, other: Self) {
        let direct_subagent_id = if self.tool_calls == 0 {
            other.direct_subagent_id.clone()
        } else if other.tool_calls == 0 {
            self.direct_subagent_id.clone()
        } else {
            None
        };
        self.tool_calls += other.tool_calls;
        self.direct_subagent_id = if self.tool_calls == 1 {
            direct_subagent_id
        } else {
            None
        };
        self.searched_patterns += other.searched_patterns;
        self.searched_file_patterns += other.searched_file_patterns;
        self.read_files += other.read_files;
        self.shell_commands += other.shell_commands;
        self.edited_files += other.edited_files;
        self.wrote_files += other.wrote_files;
        self.agents += other.agents;
        self.linked_agents += other.linked_agents;
        self.agent_descriptions.extend(other.agent_descriptions);
        self.fetched_urls += other.fetched_urls;
        self.web_searches += other.web_searches;
        self.other_tools += other.other_tools;
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
            + self.other_tools
            == 0
    }

    /// Returns styled spans: non-agent parts in dim tool_text, agent parts in accent colour.
    fn styled_spans(&self) -> Vec<(String, LineStyle)> {
        let dim = LineStyle {
            fg: Some(th().tool_text),
            dimmed: true,
            ..Default::default()
        };
        let accent = LineStyle {
            fg: Some(th().accent),
            ..Default::default()
        };

        let mut dim_parts = Vec::new();
        push_summary_item(
            &mut dim_parts,
            self.searched_patterns,
            "Searched for",
            "pattern",
        );
        push_summary_item(
            &mut dim_parts,
            self.searched_file_patterns,
            "Searched for",
            "file pattern",
        );
        push_summary_item(&mut dim_parts, self.read_files, "read", "file");
        push_summary_item(&mut dim_parts, self.shell_commands, "ran", "shell command");
        push_summary_item(&mut dim_parts, self.edited_files, "edited", "file");
        push_summary_item(&mut dim_parts, self.wrote_files, "wrote", "file");
        push_summary_item(&mut dim_parts, self.fetched_urls, "fetched", "URL");
        push_summary_item(&mut dim_parts, self.web_searches, "searched", "web");
        push_summary_item(&mut dim_parts, self.other_tools, "called", "tool");

        let agent_text = if self.agents == 0 {
            None
        } else if self.direct_subagent_id.is_some() {
            if self.agent_descriptions.len() == 1 {
                Some(format!("[→ subagent] {}", self.agent_descriptions[0]))
            } else {
                Some("[→ subagent] dispatched agent".to_string())
            }
        } else if self.linked_agents == 0 {
            if self.agents == 1 && self.agent_descriptions.len() == 1 {
                Some(format!("dispatched agent: {}", self.agent_descriptions[0]))
            } else {
                let suffix = if self.agents == 1 { "" } else { "s" };
                Some(format!("dispatched {} agent{}", self.agents, suffix))
            }
        } else if self.linked_agents == self.agents {
            let suffix = if self.agents == 1 { "" } else { "s" };
            Some(format!("dispatched {} agent{}", self.agents, suffix))
        } else {
            let suffix = if self.linked_agents == 1 { "" } else { "s" };
            Some(format!(
                "{} linked agent{} ({} dispatched)",
                self.linked_agents, suffix, self.agents
            ))
        };

        let mut spans = Vec::new();

        if !dim_parts.is_empty() {
            let mut text = capitalize_first(dim_parts.join(", "));
            if agent_text.is_some() {
                text.push_str(", ");
            }
            spans.push((text, dim));
        }

        if let Some(mut agent) = agent_text {
            if dim_parts.is_empty() {
                agent = capitalize_first(agent);
            }
            spans.push((agent, accent));
        }

        spans
    }
    pub(super) fn direct_subagent_id(&self) -> Option<&str> {
        self.direct_subagent_id.as_deref()
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
    interaction: ToolActivitySummaryInteraction<'_>,
) {
    if summary.is_empty() {
        return;
    }

    let content = summary.styled_spans();
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
            tool_output_id: interaction.tool_output_id,
            clickable: interaction.tool_output_id.is_some(),
        },
        content,
    );
    if let Some(subagent_tool_use_id) = interaction.subagent_tool_use_id
        && let Some(line) = lines.last_mut()
    {
        line.subagent_tool_use_id = Some(subagent_tool_use_id.to_string());
    }
}

pub(super) fn summarize_tool_calls(
    blocks: &[ContentBlock],
    options: &RenderOptions,
) -> ToolActivitySummary {
    let mut summary = ToolActivitySummary::default();
    for block in blocks {
        if let ContentBlock::ToolUse { id, name, input } = block {
            summary.add_call(name, id, Some(input), has_subagent_link(name, id, options));
        }
    }
    summary
}

fn assistant_blocks_are_tool_only(blocks: &[ContentBlock]) -> bool {
    blocks
        .iter()
        .all(|block| matches!(block, ContentBlock::ToolUse { .. }))
}

pub(super) fn tool_only_assistant_summary<'a>(
    entry: &'a LogEntry,
    options: &RenderOptions,
) -> Option<(Option<&'a str>, Option<&'a str>, ToolActivitySummary)> {
    let LogEntry::Assistant {
        message,
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
    if message.content.is_empty() || !assistant_blocks_are_tool_only(&message.content) {
        return None;
    }

    let summary = summarize_tool_calls(&message.content, options);
    (!summary.is_empty()).then_some((parent_tool_use_id.as_deref(), timestamp.as_deref(), summary))
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
    let label = assistant_label(pending.parent_id.as_deref());
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
                        let has_subagent = has_subagent_link(name, id, options);
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
                                subagent_tool_use_id: has_subagent.then_some(id.as_str()),
                                expanded,
                                has_subagent,
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
    if let Some(line) = lines.get_mut(first_line)
        && !line.clickable
    {
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
    let label = assistant_label(pending.parent_id.as_deref());
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
            ToolActivitySummaryInteraction {
                tool_output_id: Some(&pending.id),
                subagent_tool_use_id: pending.summary.direct_subagent_id(),
            },
        );
    }

    let end_line = lines.len();
    if end_line > start_line {
        let last_entry_index = entries[pending.last_parsed_idx].entry_index;
        messages.push(MessageRange {
            entry_index: pending.first_entry_index,
            last_entry_index,
            start_line,
            end_line,
        });
        lines.push(RenderedLine::new(vec![]));
    }
}
