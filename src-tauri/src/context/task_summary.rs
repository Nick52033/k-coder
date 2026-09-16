//! Bounded task-end compression.
//!
//! Design §4.3 allows working memory and experience to describe a finished task, but only as a
//! *structured summary*: "只保存结构化摘要，不保存完整 Provider 消息和完整命令输出". This module is
//! the single place that turns task evidence into that summary, so the guarantees live in one
//! function instead of being re-implemented by every caller:
//!
//! * Image payloads never survive. A `data:image/...;base64,...` run collapses to `[image]`, because
//!   a text summary is not a place to smuggle pixels.
//! * Credentials never survive. Every field is passed through the runtime redactor.
//! * Nothing is unbounded. Each field, each list and the rendered summary all have hard character
//!   caps, so a tool that printed 40 MiB of output can contribute a bounded line and nothing more.
//!
//! `TaskSummary` is a plain value type with no storage and no I/O, which is what makes it testable
//! and lets Task 4's maintenance turn consume it without reaching back into the runtime.

use crate::execution::redact;

use super::assembler::bound_chars;

/// Hard cap for the rendered summary. Working memory is injected into prompts, so it must stay small
/// enough that a handful of summaries cannot crowd out the request.
pub const MAX_TASK_SUMMARY_CHARS: usize = 900;
/// Per-field cap before rendering.
const MAX_FIELD_CHARS: usize = 240;
/// Per-list cap. Beyond this the summary stops being a summary.
const MAX_LIST_ITEMS: usize = 6;
/// How many lines of a tool observation a single step may quote.
const MAX_OBSERVATION_LINES: usize = 2;

/// A bounded, host-owned description of a finished task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskSummary {
    pub title: String,
    pub outcome: String,
    pub steps: Vec<String>,
    pub files: Vec<String>,
    pub follow_ups: Vec<String>,
}

impl TaskSummary {
    pub fn new(title: &str, outcome: &str) -> Self {
        Self {
            title: sanitize(title),
            outcome: sanitize(outcome),
            ..Self::default()
        }
    }

    pub fn push_step(&mut self, step: &str) {
        push_bounded(&mut self.steps, sanitize(step));
    }

    /// Records a tool observation as a bounded step.
    ///
    /// Only the first [`MAX_OBSERVATION_LINES`] non-empty lines are quoted, and the result still goes
    /// through [`sanitize`], so a full command output can never be copied into the summary.
    pub fn push_tool_step(&mut self, tool: &str, success: bool, output: &str) {
        let quoted = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(MAX_OBSERVATION_LINES)
            .collect::<Vec<_>>()
            .join(" | ");
        let status = if success { "ok" } else { "failed" };
        let step = if quoted.is_empty() {
            format!("{tool} ({status})")
        } else {
            format!("{tool} ({status}): {quoted}")
        };
        self.push_step(&step);
    }

    pub fn push_file(&mut self, path: &str) {
        let path = sanitize(path);
        if path.is_empty() || self.files.contains(&path) {
            return;
        }
        push_bounded(&mut self.files, path);
    }

    pub fn push_follow_up(&mut self, item: &str) {
        push_bounded(&mut self.follow_ups, sanitize(item));
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.outcome.is_empty()
            && self.steps.is_empty()
            && self.files.is_empty()
            && self.follow_ups.is_empty()
    }

    /// Renders the bounded summary, or `None` when there is nothing worth remembering.
    ///
    /// The output is deterministic: the same value always renders the same string, and every list
    /// keeps its insertion order.
    pub fn compress(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut sections = Vec::<String>::new();
        if !self.title.is_empty() {
            sections.push(format!("任务：{}", self.title));
        }
        if !self.outcome.is_empty() {
            sections.push(format!("结果：{}", self.outcome));
        }
        if !self.steps.is_empty() {
            let steps = self
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| format!("{}. {step}", index + 1))
                .collect::<Vec<_>>()
                .join("；");
            sections.push(format!("步骤：{steps}"));
        }
        if !self.files.is_empty() {
            sections.push(format!("文件：{}", self.files.join("、")));
        }
        if !self.follow_ups.is_empty() {
            sections.push(format!("后续：{}", self.follow_ups.join("；")));
        }
        Some(bound_chars(&sections.join("\n"), MAX_TASK_SUMMARY_CHARS))
    }
}

fn push_bounded(list: &mut Vec<String>, value: String) {
    if value.is_empty() {
        return;
    }
    if list.contains(&value) {
        return;
    }
    if list.len() == MAX_LIST_ITEMS {
        return;
    }
    list.push(value);
}

/// Removes image payloads, redacts credentials, collapses whitespace and applies the field cap.
pub fn sanitize(value: &str) -> String {
    let collapsed = strip_image_payloads(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    bound_chars(&redact(&collapsed), MAX_FIELD_CHARS)
}

/// Collapses `data:image/...;base64,...` runs so pixel data cannot ride along inside a text summary.
fn strip_image_payloads(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find("data:") {
        let (head, tail) = rest.split_at(index);
        if !starts_with_image_media_type(tail) {
            let keep = "data:".len();
            result.push_str(head);
            result.push_str(&tail[..keep]);
            rest = &tail[keep..];
            continue;
        }
        result.push_str(head);
        result.push_str("[image]");
        // The payload runs to the next whitespace; anything after it is ordinary text again.
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        rest = &tail[end..];
    }
    result.push_str(rest);
    result
}

fn starts_with_image_media_type(value: &str) -> bool {
    let prefix = value
        .chars()
        .take("data:image/".len())
        .collect::<String>()
        .to_ascii_lowercase();
    prefix == "data:image/"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_renders_a_deterministic_bounded_summary() {
        let mut summary = TaskSummary::new("修复构建", "构建通过");
        summary.push_step("定位失败用例");
        summary.push_step("修正类型导入");
        summary.push_file("src/api/runtime.ts");
        summary.push_file("src/api/runtime.ts");
        summary.push_file("src/types/runtime.ts");
        summary.push_follow_up("补原生验收");

        let rendered = summary.compress().expect("summary");
        assert_eq!(
            rendered,
            "任务：修复构建\n结果：构建通过\n步骤：1. 定位失败用例；2. 修正类型导入\n文件：src/api/runtime.ts、src/types/runtime.ts\n后续：补原生验收"
        );
        // Duplicate files are recorded once.
        assert_eq!(rendered.matches("src/api/runtime.ts").count(), 1);
        // Same value, same output.
        assert_eq!(summary.compress(), summary.compress());
    }

    #[test]
    fn empty_summary_compresses_to_nothing() {
        assert!(TaskSummary::default().compress().is_none());
        let blank = TaskSummary::new("   ", "");
        assert!(blank.is_empty());
        assert!(blank.compress().is_none());
    }

    #[test]
    fn image_payloads_are_collapsed_instead_of_copied() {
        let payload = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB";
        let value = format!("截图已保存 {payload} 供参考");
        let sanitized = sanitize(&value);
        assert_eq!(sanitized, "截图已保存 [image] 供参考");
        assert!(!sanitized.contains("base64"));
        assert!(!sanitized.contains("iVBORw0KGgo"));

        let mut summary = TaskSummary::new("上传截图", "已发送");
        summary.push_step(&format!("附件 {payload}"));
        let rendered = summary.compress().expect("summary");
        assert!(!rendered.contains("base64"));
        assert!(rendered.contains("[image]"));

        // Non-image data URLs are left alone; only pixel payloads are stripped.
        assert!(sanitize("data:text/plain;base64,aGVsbG8=").contains("data:text/plain"));
    }

    #[test]
    fn credentials_are_redacted_before_they_reach_work_memory() {
        let mut summary = TaskSummary::new("配置密钥", "完成");
        summary.push_step("API_KEY=sk-live-abcdefghijklmnop 已写入凭据槽");
        let rendered = summary.compress().expect("summary");
        // The runtime redactor keeps the key name and removes the value; what matters is that the
        // secret itself never reaches work memory or a Provider request.
        assert!(!rendered.contains("sk-live"));
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("已写入凭据槽"));
    }

    #[test]
    fn full_tool_output_is_never_copied() {
        let output = (1..=5_000)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut summary = TaskSummary::new("运行测试", "失败");
        summary.push_tool_step("run_command", false, &output);
        let rendered = summary.compress().expect("summary");
        assert!(rendered.contains("run_command (failed)"));
        assert!(rendered.contains("line 1 | line 2"));
        assert!(!rendered.contains("line 3"));
        assert!(!rendered.contains("line 5000"));
        assert!(rendered.chars().count() <= MAX_TASK_SUMMARY_CHARS);
    }

    #[test]
    fn every_field_and_list_is_bounded() {
        let mut summary = TaskSummary::new(&"汉".repeat(1_000), &"结".repeat(1_000));
        for index in 0..50 {
            summary.push_step(&format!("步骤 {index} {}", "长".repeat(500)));
            summary.push_follow_up(&format!("后续 {index}"));
        }
        assert_eq!(summary.steps.len(), MAX_LIST_ITEMS);
        assert_eq!(summary.follow_ups.len(), MAX_LIST_ITEMS);
        // `bound_chars` keeps the cap plus the truncation ellipsis.
        assert!(summary.steps[0].chars().count() <= MAX_FIELD_CHARS + 1);
        let rendered = summary.compress().expect("summary");
        assert!(rendered.chars().count() <= MAX_TASK_SUMMARY_CHARS + 1);
    }

    #[test]
    fn whitespace_is_collapsed_so_a_summary_stays_one_line_per_section() {
        let sanitized = sanitize("第一行\r\n\r\n\t第二行   第三行");
        assert_eq!(sanitized, "第一行 第二行 第三行");
    }

    #[test]
    fn task_summary_fragments_land_in_the_work_state_tier() {
        use super::super::assembler::{ContextAssembler, ContextTier};
        let mut summary = TaskSummary::new("迁移 schema", "完成");
        summary.push_step("追加 v10 迁移");
        let text = summary.compress().expect("summary");
        let fragment = super::super::assembler::ContextFragment::task_summary(
            "mem-work-1",
            "thread:t-1",
            text,
        );
        assert_eq!(fragment.tier, ContextTier::WorkStateMemory);
        assert_eq!(fragment.source.scope.as_deref(), Some("thread:t-1"));
        let assembled = ContextAssembler::default().assemble(vec![fragment]);
        assert!(assembled.render().contains("任务：迁移 schema"));
        assert!(assembled.render().starts_with("[work_state_memory]"));
    }
}
