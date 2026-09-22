const GENERIC_REASONING_NOTE_PATTERN = /^(?:(?:planning|preparing|checking|reviewing|inspecting|running|reading|analyzing|investigating|updating|implementing|verifying|testing|searching|exploring|gathering|examining|assessing|comparing|confirming|fixing|editing|applying|building|waiting)\b|(?:(?:正在|准备|计划|将要)?(?:规划|计划|准备|检查|查看|读取|运行|执行|验证|测试|搜索|分析|调查|更新|修改|修复|构建|等待)))/i;
const REASONING_INSIGHT_PATTERN = /(?:found|confirmed|because|therefore|however|mismatch|failed|failure|risk|requires?|needs?|发现|确认|原因|因此|由于|但是|不一致|失败|风险|需要)/i;
const HAN_SCRIPT_PATTERN = /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/g;
const LATIN_SCRIPT_PATTERN = /[a-z]/gi;

export function isDisplayableReasoningSummary(summary: string) {
  const normalized = summary.replace(/\s+/g, " ").trim().replace(/^[#>*_`\-\s]+/, "");
  if (!normalized) return false;
  if (!usesChineseSummaryLanguage(normalized)) return false;
  if (summary.includes("\n") || normalized.length > 160 || REASONING_INSIGHT_PATTERN.test(normalized)) return true;
  return !GENERIC_REASONING_NOTE_PATTERN.test(normalized);
}

/**
 * Keep the last summary that was safe to show while a provider continues streaming.
 * A later generic/action-only suffix must not make an already visible row disappear.
 */
export function selectVisibleReasoningSummary(summary: string, previousVisibleSummary?: string) {
  return isDisplayableReasoningSummary(summary) ? summary : previousVisibleSummary;
}

function usesChineseSummaryLanguage(summary: string) {
  const hanCount = summary.match(HAN_SCRIPT_PATTERN)?.length ?? 0;
  const latinCount = summary.match(LATIN_SCRIPT_PATTERN)?.length ?? 0;
  return hanCount >= 2 && hanCount * 4 >= latinCount;
}
