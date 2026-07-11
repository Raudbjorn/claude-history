use super::ToolOutputId;

/// A single rendered line with its spans
#[derive(Clone, Debug)]
pub struct RenderedLine {
    pub spans: Vec<(String, LineStyle)>,
    pub tool_output_id: Option<ToolOutputId>,
    /// Linked subagent target, present only on the header or direct-summary row.
    pub subagent_tool_use_id: Option<String>,
    pub clickable: bool,
}

impl RenderedLine {
    pub fn new(spans: Vec<(String, LineStyle)>) -> Self {
        Self {
            spans,
            tool_output_id: None,
            subagent_tool_use_id: None,
            clickable: false,
        }
    }

    pub fn tool_output(
        spans: Vec<(String, LineStyle)>,
        tool_output_id: ToolOutputId,
        clickable: bool,
    ) -> Self {
        Self {
            spans,
            tool_output_id: Some(tool_output_id),
            subagent_tool_use_id: None,
            clickable,
        }
    }
}

/// Style information for a span
#[derive(Clone, Debug, Default)]
pub struct LineStyle {
    pub fg: Option<(u8, u8, u8)>,
    pub bold: bool,
    pub dimmed: bool,
    pub italic: bool,
}
