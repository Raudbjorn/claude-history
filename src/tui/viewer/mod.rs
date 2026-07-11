//! Conversation viewer rendering for TUI display.
//!
//! This module renders conversation JSONL files to `Vec<RenderedLine>` for display
//! in the TUI viewer. It produces styled spans that ratatui can render directly,
//! without using ANSI escape codes.

use crate::claude::{ContentBlock, LogEntry};
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::tui::theme::{self, Theme};

mod commands;
mod entry;

pub(crate) use commands::process_command_message;
mod ledger;
mod markdown;
mod output;
mod style;
mod summary;
mod timing;
mod tools;

pub use output::{LineStyle, RenderedLine};

use entry::render_entry;
use summary::{
    PendingToolSummary, flush_tool_summary, tool_only_assistant_summary,
    user_entry_is_only_tool_results,
};
use tools::make_tool_summary_output_id;

/// Width of the focus gutter indicator (▌ + space)
pub const GUTTER_WIDTH: usize = 2;

const NAME_WIDTH: usize = 9;
/// Width of timestamp prefix when timing is enabled (space + HH:MM + space)
const TIMESTAMP_WIDTH: usize = 7;

/// Get the current theme (cached after first detection)
fn th() -> &'static Theme {
    theme::detect_theme()
}

/// Maximum body lines shown in truncated tool call mode
const TRUNCATED_BODY_LINES: usize = 3;
/// Maximum result lines shown in truncated tool result mode
const TRUNCATED_RESULT_LINES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolOutputId(pub String);

/// Controls how tool calls and results are displayed
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolDisplayMode {
    #[default]
    Hidden,
    Truncated,
    Full,
}

impl ToolDisplayMode {
    /// Cycle to the next mode: Summary → Truncated → Full → Summary
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Truncated,
            Self::Truncated => Self::Full,
            Self::Full => Self::Hidden,
        }
    }

    pub fn is_summary(self) -> bool {
        matches!(self, Self::Hidden)
    }

    /// Whether full or truncated tool details should be rendered
    pub fn shows_details(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// Whether tools should be included in exported text
    pub fn is_visible(self) -> bool {
        self.shows_details()
    }

    /// Fixed-width label for the status bar (3 chars each)
    pub fn status_label(self) -> &'static str {
        match self {
            Self::Hidden => "sum",
            Self::Truncated => "trn",
            Self::Full => "all",
        }
    }
}

/// Options for rendering a conversation
pub struct RenderOptions {
    pub tool_display: ToolDisplayMode,
    pub show_thinking: bool,
    pub show_timing: bool,
    pub content_width: usize,
    pub expanded_tool_outputs: BTreeSet<ToolOutputId>,
    /// IDs of Task or Agent tool calls that have a linked subagent file.
    pub subagent_link_ids: HashSet<String>,
}

pub(super) fn has_subagent_link(name: &str, id: &str, options: &RenderOptions) -> bool {
    matches!(name, "Task" | "Agent") && options.subagent_link_ids.contains(id)
}

/// Tracks the line range of a single message (User or Assistant entry) in the rendered output
#[derive(Clone, Debug)]
pub struct MessageRange {
    /// Index of the first JSONL entry (0-based, counting only parsed entries)
    pub entry_index: usize,
    /// Index of the last JSONL entry in this range (== entry_index for single entries;
    /// may be larger for summary groups that span multiple consecutive tool-only turns)
    pub last_entry_index: usize,
    /// Start line in rendered output (inclusive)
    pub start_line: usize,
    /// End line in rendered output (exclusive, excludes trailing blank)
    pub end_line: usize,
}

/// Result of rendering a conversation
pub struct RenderedConversation {
    pub lines: Vec<RenderedLine>,
    pub messages: Vec<MessageRange>,
}

/// Format an ISO 8601 timestamp to HH:MM local time
fn format_timestamp(iso_timestamp: &str) -> Option<String> {
    use chrono::{DateTime, Local};
    // Parse RFC 3339 timestamp (handles timezone offsets) and convert to local time
    DateTime::parse_from_rfc3339(iso_timestamp)
        .ok()
        .map(|dt| dt.with_timezone(&Local).format("%H:%M").to_string())
}

#[derive(Debug)]
pub struct RenderableEntry {
    pub entry_index: usize,
    entry: LogEntry,
}

impl RenderableEntry {
    /// Public accessor for the underlying log entry.
    pub fn entry(&self) -> &LogEntry {
        &self.entry
    }
}

pub fn parse_conversation_file(file_path: &Path) -> std::io::Result<Vec<RenderableEntry>> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut entry_index: usize = 0;

    for line_result in reader.lines() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
            if !matches!(entry, LogEntry::FileHistorySnapshot { .. }) {
                entries.push(RenderableEntry { entry_index, entry });
            }
            entry_index += 1;
        }
    }

    Ok(entries)
}

pub fn render_parsed_conversation(
    entries: &[RenderableEntry],
    options: &RenderOptions,
) -> RenderedConversation {
    let mut lines = Vec::new();
    let mut messages = Vec::new();
    let mut pending_tool_summary: Option<PendingToolSummary> = None;

    for (parsed_idx, parsed) in entries.iter().enumerate() {
        if options.tool_display.is_summary()
            && try_extend_or_start_pending_summary(
                &mut lines,
                &mut messages,
                &mut pending_tool_summary,
                entries,
                parsed_idx,
                options,
            )
        {
            continue;
        }

        flush_tool_summary(
            &mut lines,
            &mut messages,
            &mut pending_tool_summary,
            entries,
            options,
        );

        render_entry_with_range(&mut lines, &mut messages, parsed, options);
    }

    flush_tool_summary(
        &mut lines,
        &mut messages,
        &mut pending_tool_summary,
        entries,
        options,
    );

    postprocess_blank_lines(&mut lines, &mut messages);

    RenderedConversation { lines, messages }
}

/// Handle a parsed entry while in summary tool-display mode.
///
/// Returns `true` when the entry was absorbed into (or started) a pending
/// summary group and should be skipped by the normal render path.
fn try_extend_or_start_pending_summary(
    lines: &mut Vec<RenderedLine>,
    messages: &mut Vec<MessageRange>,
    pending: &mut Option<PendingToolSummary>,
    entries: &[RenderableEntry],
    parsed_idx: usize,
    options: &RenderOptions,
) -> bool {
    let parsed = &entries[parsed_idx];
    let entry_index = parsed.entry_index;
    let entry = &parsed.entry;

    if let Some((parent_id, timestamp, summary)) = tool_only_assistant_summary(entry, options) {
        match pending {
            Some(p) if p.parent_id.as_deref() == parent_id => {
                p.last_parsed_idx = parsed_idx;
                p.summary.merge(summary);
            }
            _ => {
                flush_tool_summary(lines, messages, pending, entries, options);
                *pending = Some(PendingToolSummary {
                    id: make_tool_summary_output_id(entry_index, parent_id),
                    first_entry_index: entry_index,
                    first_parsed_idx: parsed_idx,
                    last_parsed_idx: parsed_idx,
                    parent_id: parent_id.map(str::to_string),
                    timestamp: timestamp.map(str::to_string),
                    summary,
                });
            }
        }
        return true;
    }

    if user_entry_is_only_tool_results(entry, options) {
        if let Some(p) = pending {
            p.last_parsed_idx = parsed_idx;
        }
        return true;
    }

    false
}

/// Render one parsed entry and, if it produced a navigable message,
/// append a `MessageRange` that excludes any trailing blank line.
fn render_entry_with_range(
    lines: &mut Vec<RenderedLine>,
    messages: &mut Vec<MessageRange>,
    parsed: &RenderableEntry,
    options: &RenderOptions,
) {
    let entry_index = parsed.entry_index;
    let entry = &parsed.entry;
    let is_message = matches!(entry, LogEntry::User { .. } | LogEntry::Assistant { .. })
        || matches!(entry, LogEntry::Progress { data, .. }
            if options.show_thinking && crate::claude::parse_agent_progress(data).is_some());

    let start_line = lines.len();
    render_entry(lines, entry_index, entry, options);
    let end_line = lines.len();

    if !is_message {
        return;
    }
    if let Some(range) = message_range_excluding_trailing_blank(
        lines,
        start_line,
        end_line,
        entry_index,
        entry_index,
    ) {
        messages.push(range);
    }
}

/// If the rendered slice produced any non-blank lines, return a
/// `MessageRange` whose `end_line` excludes a trailing blank.
fn message_range_excluding_trailing_blank(
    lines: &[RenderedLine],
    start_line: usize,
    end_line: usize,
    entry_index: usize,
    last_entry_index: usize,
) -> Option<MessageRange> {
    if end_line <= start_line {
        return None;
    }
    let effective_end = if lines.get(end_line - 1).is_some_and(|l| l.spans.is_empty()) {
        end_line - 1
    } else {
        end_line
    };
    if effective_end <= start_line {
        return None;
    }
    Some(MessageRange {
        entry_index,
        last_entry_index,
        start_line,
        end_line: effective_end,
    })
}

/// Collapse consecutive blank rendered lines and remap message ranges so
/// they continue to point at their original visible content.
///
/// Multiple render helpers each push a trailing blank line, which can
/// produce adjacent blanks when a tool result emits empty output. The
/// dedup pass removes any blank line whose immediate predecessor is also
/// blank, and the remap pass shifts every range start/end onto the new
/// line indices, clamping ranges that ended on a removed blank.
fn postprocess_blank_lines(lines: &mut Vec<RenderedLine>, messages: &mut Vec<MessageRange>) {
    let mut removed = vec![false; lines.len()];
    let mut i = 1;
    while i < lines.len() {
        if lines[i].spans.is_empty() && lines[i - 1].spans.is_empty() {
            removed[i] = true;
        }
        i += 1;
    }

    // Build index mapping: old line index -> new line index. Removed
    // entries get the index they would collapse onto; they are never
    // dereferenced for surviving ranges because the remap below walks
    // backward off any removed terminator first.
    let mut new_index = Vec::with_capacity(lines.len());
    let mut offset = 0usize;
    for (idx, &is_removed) in removed.iter().enumerate() {
        if is_removed {
            new_index.push(idx - offset);
            offset += 1;
        } else {
            new_index.push(idx - offset);
        }
    }
    let total_after = lines.len() - offset;

    // Compact in place.
    let mut write = 0;
    for (read, &is_removed) in removed.iter().enumerate() {
        if !is_removed {
            if write != read {
                lines.swap(write, read);
            }
            write += 1;
        }
    }
    lines.truncate(total_after);

    for msg in messages.iter_mut() {
        msg.start_line = new_index[msg.start_line];
        if msg.end_line > 0 && msg.end_line <= new_index.len() {
            // end_line is exclusive — find the new index of the last
            // non-removed line before it and add 1.
            let mut last = msg.end_line - 1;
            while last > msg.start_line && removed[last] {
                last -= 1;
            }
            msg.end_line = new_index[last] + 1;
        } else if msg.end_line == new_index.len() {
            msg.end_line = total_after;
        }
        msg.end_line = msg.end_line.min(total_after);
        msg.start_line = msg.start_line.min(msg.end_line);
    }

    messages.retain(|m| m.start_line < m.end_line);
}

/// Render a conversation file to lines for display in the TUI viewer
pub fn render_conversation(
    file_path: &Path,
    options: &RenderOptions,
) -> std::io::Result<RenderedConversation> {
    let entries = parse_conversation_file(file_path)?;
    Ok(render_parsed_conversation(&entries, options))
}

// Called by enter_view_mode and push_view (added in subsequent tasks).
/// Discover subagent JSONL files for Task/Agent tool calls in a conversation.
///
/// Agent files live at `<project_dir>/<session_uuid>/subagents/agent-<id>.jsonl`.
/// Two matching strategies:
///   - Task calls: file_id is a prefix of the tool_use_id (legacy format)
///   - Agent calls: match by description field in agent-<id>.meta.json
///
/// Returns a map from tool_use_id → agent file path.
#[allow(dead_code)]
pub fn scan_subagent_dir(
    jsonl_path: &Path,
    entries: &[RenderableEntry],
) -> HashMap<String, PathBuf> {
    let mut links = HashMap::new();

    let subagents_dir = match jsonl_path.parent() {
        Some(parent) if parent.file_name().and_then(|name| name.to_str()) == Some("subagents") => {
            parent.to_path_buf()
        }
        Some(project_dir) => {
            let Some(session_uuid) = jsonl_path.file_stem().and_then(|stem| stem.to_str()) else {
                return links;
            };
            project_dir.join(session_uuid).join("subagents")
        }
        None => return links,
    };

    let read_dir = match std::fs::read_dir(&subagents_dir) {
        Ok(d) => d,
        Err(_) => return links, // directory absent → no subagents
    };

    struct AgentFile {
        file_id: String,
        tool_use_id: Option<String>,
        description: Option<String>,
        path: PathBuf,
    }

    // Collect agent files with metadata parsed from .meta.json
    let agent_files: Vec<AgentFile> = read_dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !name.starts_with("agent-") || !name.ends_with(".jsonl") {
                return None;
            }
            let file_id = name["agent-".len()..name.len() - ".jsonl".len()].to_string();
            let stem = path.file_stem()?.to_str()?.to_string();
            let meta_path = path.parent()?.join(format!("{}.meta.json", stem));
            let meta = std::fs::read_to_string(meta_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            let tool_use_id = meta
                .as_ref()
                .and_then(|v| v["toolUseId"].as_str().map(|s| s.to_string()));
            let description = meta
                .as_ref()
                .and_then(|v| v["description"].as_str().map(|s| s.to_string()));
            Some(AgentFile {
                file_id,
                tool_use_id,
                description,
                path,
            })
        })
        .collect();

    // toolUseId → path (exact match, preferred for Agent calls)
    let id_to_path: HashMap<&str, &PathBuf> = agent_files
        .iter()
        .filter_map(|af| af.tool_use_id.as_deref().map(|id| (id, &af.path)))
        .collect();

    // Description fallback is safe only when exactly one agent file has that description.
    let mut desc_to_path: HashMap<&str, Option<&PathBuf>> = HashMap::new();
    for agent_file in &agent_files {
        if let Some(description) = agent_file.description.as_deref() {
            desc_to_path
                .entry(description)
                .and_modify(|path| *path = None)
                .or_insert(Some(&agent_file.path));
        }
    }

    for renderable in entries {
        if let LogEntry::Assistant { message, .. } = &renderable.entry {
            for block in &message.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    match name.as_str() {
                        "Task" => {
                            // file_id is a prefix of the full tool_use_id
                            // Collect all candidates where tool_use_id starts with file_id
                            let candidates: Vec<&AgentFile> = agent_files
                                .iter()
                                .filter(|af| id.starts_with(af.file_id.as_str()))
                                .collect();

                            match candidates.len() {
                                0 => {} // No match
                                1 => {
                                    links.insert(id.clone(), candidates[0].path.clone());
                                }
                                _ => {
                                    // Multiple candidates: find longest unique prefix
                                    let max_len = candidates
                                        .iter()
                                        .map(|af| af.file_id.len())
                                        .max()
                                        .unwrap_or(0);
                                    let longest: Vec<&&AgentFile> = candidates
                                        .iter()
                                        .filter(|af| af.file_id.len() == max_len)
                                        .collect();

                                    if longest.len() == 1 {
                                        links.insert(id.clone(), longest[0].path.clone());
                                    } else {
                                        // Defensive: unreachable via the public API because
                                        // read_dir produces unique file_ids (the filesystem
                                        // forbids two files with the same name). Kept as a
                                        // guard if scan_subagent_dir is ever refactored to
                                        // accept a pre-built agent list.
                                    }
                                }
                            }
                        }
                        "Agent" => {
                            // Prefer exact toolUseId match from meta.json; fall back to description
                            if let Some(&agent_path) = id_to_path.get(id.as_str()) {
                                links.insert(id.clone(), agent_path.clone());
                            } else if let Some(desc) = input["description"].as_str()
                                && let Some(agent_path) =
                                    desc_to_path.get(desc).and_then(|path| *path)
                            {
                                links.insert(id.clone(), agent_path.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    links
}

#[cfg(test)]
mod tests;
