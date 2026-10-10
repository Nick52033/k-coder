#!/usr/bin/env python3
"""k-coder 桌面端运行时错误排查辅助脚本。

用法（Git Bash 或 PowerShell 均可，python 为 Windows 版）：
  python triage_runtime_errors.py classify [--minutes 1440] [--all] [--verbose]
      一键巡检：读 runtime.jsonl 的 error 记录，按规则表自动分类，直接输出
      「哪些是 bug 要修、哪些不是 bug」。这是本 skill 的首选入口。

  python triage_runtime_errors.py errors --minutes 30
      列出 runtime.jsonl 中最近 N 分钟的 error 级记录，按 threadId 分组，
      并标注每个会话最近活跃时间，用来定位「当前对话 / 刚失败的对话」。

  python triage_runtime_errors.py session --thread <threadId> [--since-ms 1791445000000]
      打印指定会话的压缩时间线。只保留排查需要的关键事件；item_started /
      item_completed / provider_context / reasoning_summary 默认跳过——
      未经压缩直接 dump 整个 .jsonl 会溢出上下文。

两种日志格式不同，注意别混用字段名：
  runtime.jsonl  每条是 {level, timestampMs, event, fields{...}}
  sessions/*.jsonl 每条是 {type, createdAtMs, data{...}}（schemaVersion 11）

规则表与判定口径见 SKILL.md「一键巡检」；新增类别时同步改 RULES 和 SKILL.md，
避免脚本和文档给出两套结论。
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import sys


# ── 分类规则表 ────────────────────────────────────────────────────────────
# 每条规则按顺序匹配，命中即停，所以顺序越靠前越具体。
# verdict 取值：
#   bug      产品缺陷，需要改代码
#   not-bug  按设计 / 模型侧 / 环境侧，无需改代码
#   review   证据不足或需要核对当前源码/运行版本，不能自动排除或确认缺陷

REPLACEMENT_CHAR = "\ufffd"

PS_SYNTAX_MARKERS = (
    "无法将",
    "不是此版本中的有效语句",
    "字符串缺少终止符",
    "找不到与参数名称",
    "找不到与参数转换",
    "不是有效参数",
    "无法将“",
    "无法将\"",
    "ParserError",
    "ParameterBindingException",
    "CommandNotFoundException",
    "ItemNotFoundException",
    "表达式或语句中包含意外的标记",
)

TEST_FAILURE_MARKERS = (
    "test result: FAILED",
    "panicked at",
    "AssertionError",
    "assertion failed",
    "assertion `",
    "FAILED tests/",
    "error: test failed",
)

TEST_CONTEXT_MARKERS = (
    "toBeVisible",
    "Timeout:",
    "element(s) not found",
    "Call log:",
    "Error Context:",
    "test-results\\",
    "[WebServer]",
    "playwright test",
    "[desktop] ›",
    "[narrow] ›",
    "page.evaluate",
    "locator(",
    "getByRole(",
    "expect(",
)


def has_replacement_chars(text: str) -> bool:
    """检测替换字符，只作为编码待核对信号，不推断当前解码实现。"""
    return text.count(REPLACEMENT_CHAR) >= 2


def _match(event: str, text: str, *needles: str) -> bool:
    lowered = text.lower()
    return any(needle in event or needle.lower() in lowered for needle in needles)


RULES: list[dict] = [
    {
        "id": "B1",
        "verdict": "review",
        "title": "移动网关恢复失败：需核对绑定原因和运行版本",
        "detail": "日志证明当时恢复失败，不证明当前源码仍缺少回退或提示。"
                  "应核对绑定错误、后续回退事件、修复提交与实际运行版本。",
        "evidence": "src-tauri/src/mobile/mod.rs、src-tauri/src/mobile/server.rs",
        "fix": "先确认是否已经修复并部署；仅仍可复现时修改绑定回退或错误提示。",
        "match": lambda event, text: "mobile.gateway.restore_failed" in event,
    },
    {
        "id": "R1",
        "verdict": "review",
        "title": "真实测试失败：不能作为命令噪声忽略",
        "detail": "输出包含失败断言、panic 或测试失败汇总。即使命令使用 PowerShell 管道，"
                  "也不能覆盖这些失败证据；仍需对照测试、源码和环境判断根因。",
        "evidence": "session tool_result 的完整输出、exitCode 与失败测试名",
        "fix": "定位失败测试并修复源码、夹具或过时断言，再只运行受影响测试。",
        "match_record": lambda record: has_test_failure(record),
    },
    {
        "id": "N1",
        "verdict": "not-bug",
        "title": "工作区沙箱拒绝工作区外路径",
        "detail": "按设计拒绝：含符号链接与目录联接逃逸防护，模型反复试探绝对路径属模型侧行为。",
        "evidence": "AGENTS.md「安全约束」",
        "match": lambda event, text: _match(event, text, "path must be relative"),
    },
    {
        "id": "N2",
        "verdict": "not-bug",
        "title": "本地插件未启用被拒绝",
        "detail": "按设计拒绝未启用的插件，需要用户在设置里启用后才能调用。",
        "evidence": "extensions 插件加载逻辑",
        "match": lambda event, text: "plugin" in text.lower() and "is not enabled" in text.lower(),
    },
    {
        "id": "N3",
        "verdict": "not-bug",
        "title": "补丁目标超过单文件大小硬上限",
        "detail": "有意保守：大文件必须先拆分或改用 run_command 编辑，否则模型永远学不到边界。",
        "evidence": "src-tauri/src/agent/mod.rs:1047",
        "match": lambda event, text: _match(event, text, "patch exceeds a safety limit"),
    },
    {
        "id": "N4",
        "verdict": "not-bug",
        "title": "补丁语法不合法",
        "detail": "空 hunk、上下文不匹配等模型侧构造错误，诊断信息已指引重新读取目标区域。",
        "evidence": "src-tauri/src/patch/mod.rs",
        "match": lambda event, text: _match(
            event, text,
            "patch syntax is invalid",
            "patch conflicts with the workspace",
        ),
    },
    {
        "id": "N5",
        "verdict": "not-bug",
        "title": "补丁熔断：连续恢复轮次仍未写入",
        "detail": "P10-259 有界熔断，根因是模型在超上限文件上反复打补丁，属模型策略问题。",
        "evidence": "src-tauri/src/agent/mod.rs:1052",
        "match": lambda event, text: "apply_patch_loop" in text,
    },
    {
        "id": "N6",
        "verdict": "review",
        "title": "Provider 返回 HTTP 错误：需核对账号、请求与服务状态",
        "detail": "实名认证、配额、限流通常由账号或供应商处理，但 HTTP 错误也可能由请求协议"
                  "或配置引起，不能仅凭错误正文排除产品缺陷。",
        "evidence": "fields.status / retryAfterMs 已随记录落库",
        "match": lambda event, text: _match(event, text, "provider returned http"),
    },
    {
        "id": "N7",
        "verdict": "review",
        "title": "模型空响应：需核对 Provider 用量、收尾逻辑和运行版本",
        "detail": "completed 没有可见内容可能来自模型输出预算、协议适配或旧版本行为。"
                  "需读取相关会话和当前源码，不能根据历史规则断言已有守卫或不是缺陷。",
        "evidence": "src-tauri/src/agent/mod.rs MAX_EMPTY_RESPONSE_RETRIES",
        "match": lambda event, text: "completed without text or a tool call" in text,
    },
    {
        "id": "N8",
        "verdict": "not-bug",
        "title": "PowerShell 不会展开 rg 路径通配符（预防性拦截）",
        "detail": "命令尚未执行即被拦截并给出改写建议，是有意保守，避免模型误判为无匹配。",
        "evidence": "src-tauri/src/tools/command_diagnostics.rs",
        "match": lambda event, text: "不会展开原生 rg" in text,
    },
    {
        "id": "N9",
        "verdict": "not-bug",
        "title": "PowerShell 语法或命令用法错误",
        "detail": "模型把 bash 写法（head、&&）或错误引号带到 PowerShell，属模型侧命令问题。",
        "evidence": "run_command stderr 原文",
        "match": lambda event, text: any(marker in text for marker in PS_SYNTAX_MARKERS),
    },
    {
        "id": "N10",
        "verdict": "not-bug",
        "title": "命令自身失败：路径不存在或正则不合法",
        "detail": "命令报告不存在的目录或非法正则，应修正输入；不能把这些错误当作搜索无匹配。",
        "evidence": "run_command 退出码与 stderr",
        "match": lambda event, text: _match(
            event, text,
            "系统找不到指定的文件",
            "未能找到路径",
            "regex parse error",
            "unclosed character class",
        ),
    },
    {
        "id": "N16",
        "verdict": "review",
        "title": "历史搜索未匹配提示：需核对原始结果与运行版本",
        "detail": "仅有 no matches 提示不能证明命令形态、完整空输出或 stderr 均符合条件。"
                  "当前工具对确认的无匹配返回 success=true 并保留 exitCode=1，"
                  "但历史失败日志不会因此消失，也不能据此确认安装端已修复。",
        "evidence": "session tool_result 完整输出、metadata 与原始 command",
        "fix": "核对真实退出码、命令形态及 stderr；不要将路径、正则错误或复杂脚本强制转为成功。",
        "match": lambda event, text: _match(event, text, "no matches (exit code 1)"),
    },
    {
        "id": "N11",
        "verdict": "not-bug",
        "title": "工具入参不合法",
        "detail": "模型给了不存在的路径或非法参数，诊断信息已列出可能存在的候选路径。",
        "evidence": "tool_failed fields.arguments",
        "match": lambda event, text: "invalid tool arguments" in text,
    },
    {
        "id": "N12",
        "verdict": "not-bug",
        "title": "浏览器工具只接受 http/https",
        "detail": "按设计限制协议，属调用方参数问题。",
        "evidence": "browser 工具实现",
        "match": lambda event, text: _match(event, text, "only supports http and https"),
    },
    {
        "id": "N13",
        "verdict": "review",
        "title": "测试或构建上下文：输出不足以判定根因",
        "detail": "测试命令和 locator 上下文不是成功证据。需核对完整汇总和真实退出码，"
                  "区分失败测试、基础设施错误与 Shell 状态误判。",
        "evidence": "session tool_result 完整输出与 exitCode",
        "match_record": lambda record: any(
            marker in record_output(record) for marker in TEST_CONTEXT_MARKERS
        ),
    },
    {
        "id": "N15",
        "verdict": "not-bug",
        "title": "模型把 bash heredoc 带到 PowerShell（python - <<'PY'）",
        "detail": "PowerShell 不支持 Bash 的 heredoc 写法，命令在解析期或预检被拒绝。"
                  "若输出还含替换字符，另行核对编码和运行版本，不能断言当前源码有解码缺陷。",
        "evidence": "run_command 的 PowerShell heredoc 诊断提示",
        "fix": "多行 Python 使用脚本文件，单行使用 python -c；不要原样重试。",
        "match": lambda event, text: "PowerShell 不支持 Bash 的 python" in text,
    },
    {
        "id": "N14",
        "verdict": "review",
        "title": "命令有输出但状态失败：需核对实际退出码",
        "detail": "输出、管道或 stderr 均不能单独证明成功。PowerShell 可能误报原生命令状态，"
                  "也可能保留了真实测试、IO 或语法失败；必须核对完整工具结果。",
        "evidence": "session tool_result.metadata.exitCode / state / outputChunks",
        "fix": "优先核对原程序退出码；构建测试直接运行，不追加 2>&1 或截取管道。"
               "不要自动重跑有副作用的历史命令或按输出关键词强制成功。",
        "match_record": lambda record: is_nonzero_exit_with_output(record),
    },
]


# 伴随观察独立计数，不把输出损坏直接判成当前源码缺陷或失败主因。
OBSERVATIONS: list[dict] = [
    {
        "id": "B2",
        "verdict": "review",
        "title": "命令输出含替换字符：需核对解码链路和运行版本",
        "detail": "历史输出存在 U+FFFD 替换字符，但也可能是上游输出或旧版本解码造成。"
                  "不能据此断言当前源码仍使用错误解码方式。",
        "evidence": "src-tauri/src/execution.rs OutputDecoder 与原始会话输出",
        "fix": "确认当前运行版本是否包含解码修复；仍复现时用非敏感输出定位字节来源。",
        "match": lambda event, text: has_replacement_chars(text),
    },
]

UNMATCHED: dict = {
    "id": "R0",
    "verdict": "review",
    "title": "规则未覆盖，需人工判定",
    "detail": "新出现的错误形态。先按 errors / session 子命令读取相关记录，再核对源码和运行版本。",
    "evidence": "",
    "fix": "依据根因修规则或源码，不把未判定项直接算成 bug，也不自动重跑历史命令。",
}

def as_dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def record_output(record: dict) -> str:
    return str(as_dict(record.get("fields")).get("output") or "")


def has_test_failure(record: dict) -> bool:
    return record.get("event") == "tool_failed" and any(
        marker in record_output(record) for marker in TEST_FAILURE_MARKERS
    )


def is_nonzero_exit_with_output(record: dict) -> bool:
    if record.get("event") != "tool_failed":
        return False
    fields = as_dict(record.get("fields"))
    command = as_dict(fields.get("arguments")).get("command")
    return bool(command and record_output(record))


VERDICT_LABEL = {
    "bug": "需要修复（产品缺陷）",
    "review": "待人工判定",
    "not-bug": "不是 bug（按设计 / 模型侧 / 环境侧）",
}


def record_text(record: dict, include_command: bool = True) -> str:
    fields = as_dict(record.get("fields"))
    parts = [
        str(fields.get("message") or ""),
        str(fields.get("output") or ""),
        str(fields.get("reason") or ""),
        str(fields.get("errorCode") or ""),
        str(fields.get("tool") or ""),
    ]
    arguments = fields.get("arguments")
    if include_command and isinstance(arguments, dict):
        parts.append(str(arguments.get("command") or ""))
    return "\n".join(parts)


def classify_record_with_observations(record: dict) -> tuple[dict, list[dict]]:
    """返回 (主因规则, 伴随观察)，不把历史现象当作当前源码缺陷。"""
    event = record.get("event") or ""
    text = record_text(record, include_command=False)
    primary = UNMATCHED
    for rule in RULES:
        if rule.get("match_record") is not None:
            if rule["match_record"](record):
                primary = rule
                break
        elif rule["match"](event, text):
            primary = rule
            break
    extras = [
        rule for rule in OBSERVATIONS
        if record.get("event") == "tool_failed" and rule["match"](event, record_output(record))
    ]
    return primary, extras

RUNTIME_ROOT = os.path.join(
    os.environ.get("APPDATA", os.path.expanduser("~")),
    "com.kcoder.app",
    "runtime-data",
)
LOG_PATH = os.path.join(RUNTIME_ROOT, "logs", "runtime.jsonl")
SESSION_DIR = os.path.join(RUNTIME_ROOT, "sessions")

# 会话流水里值得看的事件；其余类型对定位失败根因基本是噪声。
SESSION_EVENTS = {
    "thread_created",
    "thread_workspace_bound",
    "thread_model_selected",
    "user_message",
    "turn_started",
    "turn_failed",
    "turn_completed",
    "turn_cancelled",
    "provider_call_usage",
    "assistant_tool_calls",
    "tool_result",
    "assistant_message",
    "user_input_requested",
    "user_input_resolved",
}

MAX_TEXT = 220          # 单条文本字段的最大展示长度
MAX_EVENTS = 400        # session 模式最多打印多少条，防止失控


def fmt_time(ms: int) -> str:
    return datetime.datetime.fromtimestamp(ms / 1000).strftime("%m-%d %H:%M:%S")


def clip(text: str, limit: int = MAX_TEXT) -> str:
    text = str(text or "").replace("\r\n", "\\n").replace("\n", "\\n")
    return text if len(text) <= limit else text[: limit - 1] + "…"


def print_group(group: dict, verbose: bool = False) -> None:
    rule = group["rule"]
    records = group["records"]
    times = [r.get("timestampMs", 0) for r in records]
    threads: list[str] = []
    for record in records:
        thread = as_dict(record.get("fields")).get("threadId") or "(无 threadId)"
        if thread not in threads:
            threads.append(thread)
    tag = "（伴随观察，不重复计入主因）" if group.get("cooccurring") else ""
    print(f"\n[{rule['id']}]{tag} {rule['title']}  ×{len(records)}")
    print(f"    时间：{fmt_time(min(times))} ~ {fmt_time(max(times))}"
          f"   会话：{', '.join(threads[:3])}{' 等' if len(threads) > 3 else ''}")
    sample = clip(record_text(records[0]), 150)
    if sample:
        print(f"    现场：{sample}")
    print(f"    判定：{rule['detail']}")
    if rule.get("evidence"):
        print(f"    证据：{rule['evidence']}")
    if rule.get("fix"):
        print(f"    处理：{rule['fix']}")
    if verbose:
        for record in records:
            fields = as_dict(record.get("fields"))
            print(f"      - {fmt_time(record.get('timestampMs', 0))}"
                  f"  {record.get('event')}  {fields.get('tool') or ''}"
                  f"  {clip(record_text(record), 90)}")


def cmd_classify(args: argparse.Namespace) -> int:
    now_ms = datetime.datetime.now().timestamp() * 1000
    cutoff = 0.0 if args.all else now_ms - args.minutes * 60_000

    records: list[dict] = []
    cleared_at: int | None = None
    total = 0
    outside = 0
    lo_ms = hi_ms = 0
    with open(LOG_PATH, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(record, dict):
                continue
            total += 1
            ms = record.get("timestampMs", 0)
            lo_ms = ms if not lo_ms else min(lo_ms, ms)
            hi_ms = max(hi_ms, ms)
            if record.get("event") == "logs_cleared":
                cleared_at = ms
            if record.get("level") != "error":
                continue
            if ms < cutoff:
                outside += 1
                continue
            records.append(record)

    window = (f"全部历史（{fmt_time(lo_ms)} ~ {fmt_time(hi_ms)}）" if args.all
              else f"{fmt_time(cutoff)} ~ {fmt_time(now_ms)}（最近 {args.minutes} 分钟）")
    print(f"巡检窗口：{window}")
    print(f"日志：{LOG_PATH}")
    print(f"共扫描 {total} 条记录，其中 error {len(records) + outside} 条"
          + (f"（窗口外 {outside} 条，加 --all 看全部）" if outside else "") + "。")
    if cleared_at:
        print(f"注意：日志在 {fmt_time(cleared_at)} 被手动清空过（logs_cleared），"
              "早于该时刻的错误不在文件里，不能据此断定没发生过错误。")
    if not records:
        print("\n窗口内没有 error 级记录。")
        return 0

    groups: dict[str, dict] = {}
    observations: dict[str, dict] = {}
    classified: list[tuple[dict, dict, list[dict]]] = []
    for record in records:
        rule, extras = classify_record_with_observations(record)
        classified.append((record, rule, extras))
        groups.setdefault(rule["id"], {"rule": rule, "records": []})["records"].append(record)
        for extra in extras:
            observations.setdefault(extra["id"], {"rule": extra, "records": []})["records"].append(record)

    bugs = [g for g in groups.values() if g["rule"]["verdict"] == "bug"]
    for group in observations.values():
        group["cooccurring"] = True
        if group["rule"]["verdict"] == "bug":
            bugs.append(group)
    bugs.sort(key=lambda g: (g.get("cooccurring", False), -len(g["records"])))

    print(f"\n{'━' * 70}")
    print(f"一、{VERDICT_LABEL['bug']}：{len(bugs)} 类 / "
          f"{sum(len(g['records']) for g in bugs)} 条")
    print("━" * 70)
    if not bugs:
        print("\n（无）")
    for group in bugs:
        print_group(group, verbose=args.verbose)

    for verdict in ("review", "not-bug"):
        items = [g for g in groups.values() if g["rule"]["verdict"] == verdict]
        items.extend(g for g in observations.values() if g["rule"]["verdict"] == verdict)
        items.sort(key=lambda g: -len(g["records"]))
        if not items:
            continue
        label = "二" if verdict == "review" else "三"
        print(f"\n{'━' * 70}")
        print(f"{label}、{VERDICT_LABEL[verdict]}：{len(items)} 类 / "
              f"{sum(len(g['records']) for g in items)} 条")
        print("━" * 70)
        for group in items:
            print_group(group, verbose=args.verbose)

    primary_bugs = [g for g in groups.values() if g["rule"]["verdict"] == "bug"]
    primary_bug_n = sum(1 for _, rule, _ in classified if rule["verdict"] == "bug")
    review_n = sum(1 for _, rule, _ in classified if rule["verdict"] == "review")
    obs_n = sum(1 for _, _, extras in classified if extras)
    rest_n = sum(1 for _, rule, _ in classified if rule["verdict"] == "not-bug")
    print(f"\n{'━' * 70}")
    print("结论")
    print("━" * 70)
    if primary_bugs:
        names = "、".join(f"{g['rule']['id']}×{len(g['records'])}" for g in primary_bugs)
        print(f"· 需要修复：{primary_bug_n} 条，主因是产品缺陷（{names}）。")
    else:
        print("· 未从这些日志确认当前产品缺陷；这不等于已经排除缺陷。")
    if observations:
        names = "、".join(f"{g['rule']['id']}×{len(g['records'])}" for g in observations.values())
        print(f"· 伴随观察：{obs_n} 条（{names}），不重复计入主因；需核对当前源码与运行版本。")
    if review_n:
        print(f"· 待人工判定：{review_n} 条，核对完整会话、真实退出码和当前源码后再定性。")
    print(f"· 按设计或命令输入问题：{rest_n} 条；这些证据不要求修改运行时。")
    pointers = "、".join(
        g["rule"]["evidence"] for g in [*groups.values(), *observations.values()]
        if g["rule"]["verdict"] != "not-bug" and g["rule"].get("evidence")
    )
    if pointers:
        print(f"· 复核入口：{pointers}")
    return 0


def cmd_errors(args: argparse.Namespace) -> int:
    now_ms = datetime.datetime.now().timestamp() * 1000
    cutoff = now_ms - args.minutes * 60_000

    errors: list[dict] = []
    cleared_at: int | None = None
    with open(LOG_PATH, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(record, dict):
                continue
            if record.get("event") == "logs_cleared":
                cleared_at = record.get("timestampMs")
            if record.get("level") != "error":
                continue
            if record.get("timestampMs", 0) >= cutoff:
                errors.append(record)

    print(f"窗口：{fmt_time(cutoff)} ~ {fmt_time(now_ms)}  日志：{LOG_PATH}")
    if not errors:
        print("窗口内没有 error 级记录。")
        if cleared_at:
            print(f"注意：日志在 {fmt_time(cleared_at)} 被手动清空过（logs_cleared），"
                  "早于该时刻的错误不在文件里，不能据此断定没发生过错误。")
        return 0

    by_thread: dict[str, list[dict]] = {}
    for record in errors:
        thread = as_dict(record.get("fields")).get("threadId") or "(无 threadId)"
        by_thread.setdefault(thread, []).append(record)

    for thread, records in sorted(by_thread.items(), key=lambda kv: kv[1][0]["timestampMs"]):
        session_file = os.path.join(SESSION_DIR, f"{thread}.jsonl")
        last_active = ""
        if os.path.exists(session_file):
            last_ms = 0
            with open(session_file, encoding="utf-8") as handle:
                for line in handle:
                    try:
                        record = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if isinstance(record, dict):
                        last_ms = max(last_ms, record.get("createdAtMs", 0))
            last_active = f"  会话最近活跃 {fmt_time(last_ms)}"
        print(f"\n== thread {thread}  ({len(records)} 条 error){last_active}")
        for record in records:
            fields = as_dict(record.get("fields"))
            detail = fields.get("message") or fields.get("output") or ""
            extra = {k: v for k, v in fields.items()
                     if k not in ("threadId", "message", "output", "arguments")}
            print(f"  {fmt_time(record['timestampMs'])}  {record.get('event')}")
            print(f"    {clip(detail)}")
            if args.verbose and extra:
                print(f"    fields: {clip(json.dumps(extra, ensure_ascii=False))}")
    print(f"\n共 {len(errors)} 条 error，涉及 {len(by_thread)} 个会话。"
          "下一步：python triage_runtime_errors.py session --thread <上面的 threadId>")
    return 0


def cmd_session(args: argparse.Namespace) -> int:
    path = os.path.join(SESSION_DIR, f"{args.thread}.jsonl")
    if not os.path.exists(path):
        print(f"会话文件不存在：{path}", file=sys.stderr)
        return 1

    printed = 0
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(record, dict):
                continue
            ms = record.get("createdAtMs", 0)
            if args.since_ms and ms < args.since_ms:
                continue
            event = record.get("type")
            if event not in SESSION_EVENTS:
                continue
            data = as_dict(record.get("data"))
            print(print_session_event(ms, event, data))
            printed += 1
            if printed >= MAX_EVENTS:
                print(f"…… 已达 {MAX_EVENTS} 条上限，用 --since-ms 收窄时间范围再看剩余部分")
                break
    if printed == 0:
        print("（该窗口内没有关键事件；换 --since-ms 或去掉过滤）")
    return 0


def print_session_event(ms: int, event: str, data: dict) -> str:
    stamp = fmt_time(ms)
    data = as_dict(data)
    if event == "user_message":
        message = as_dict(data.get("message"))
        text = " ".join(
            part["text"]
            for part in (message.get("content") or [])
            if isinstance(part, dict)
            and part.get("type") == "text"
            and isinstance(part.get("text"), str)
        )
        return f"{stamp}  USER: {clip(text, 300)}"
    if event == "thread_model_selected":
        return f"{stamp}  MODEL: {data.get('model')}  (provider={data.get('provider_id')})"
    if event == "provider_call_usage":
        usage = as_dict(data.get("usage"))
        details = as_dict(data.get("details"))
        return (f"{stamp}  USAGE #{data.get('call_index')}  {data.get('model')}"
                f"  出={usage.get('outputTokens')}"
                f"  思考={details.get('reasoningOutputTokens')}")
    if event == "assistant_tool_calls":
        calls = data.get("calls") or []
        parts = []
        for call in calls:
            call = as_dict(call)
            arguments = as_dict(call.get("arguments"))
            brief = arguments.get("command") or arguments.get("path") or json.dumps(
                arguments, ensure_ascii=False)
            parts.append(f"{call.get('name')}({clip(brief, 160)})")
        return f"{stamp}  CALL: " + " | ".join(parts)
    if event == "tool_result":
        result = as_dict(data.get("result"))
        meta = as_dict(result.get("metadata"))
        return (f"{stamp}  RESULT {data.get('name')} {data.get('call_id')}"
                f"  success={result.get('success')} exit={meta.get('exitCode')}"
                f"  out={clip(result.get('output') or '', 160)}")
    if event == "assistant_message":
        message = as_dict(data.get("message"))
        text = " ".join(
            part["text"]
            for part in (message.get("content") or [])
            if isinstance(part, dict)
            and part.get("type") == "text"
            and isinstance(part.get("text"), str)
        )
        return f"{stamp}  ASSISTANT: {clip(text, 300)}"
    return f"{stamp}  {event}: {clip(json.dumps(data, ensure_ascii=False))}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)

    classify = sub.add_parser("classify", help="一键巡检：自动分类 error 记录并给出是否 bug 的结论")
    classify.add_argument("--minutes", type=int, default=1440,
                          help="时间窗（分钟），默认 1440 即 24 小时")
    classify.add_argument("--all", action="store_true", help="扫全部历史，忽略时间窗")
    classify.add_argument("--verbose", action="store_true", help="每组附注每条命中记录")
    classify.set_defaults(func=cmd_classify)

    errors = sub.add_parser("errors", help="时间窗内的 error 记录，按会话分组")
    errors.add_argument("--minutes", type=int, default=30)
    errors.add_argument("--verbose", action="store_true", help="附带打印 fields 全量字段")
    errors.set_defaults(func=cmd_errors)

    session = sub.add_parser("session", help="打印指定会话的压缩时间线")
    session.add_argument("--thread", required=True)
    session.add_argument("--since-ms", type=int, default=0,
                         help="只要 createdAtMs >= 该值的事件（epoch 毫秒）")
    session.set_defaults(func=cmd_session)

    args = parser.parse_args()
    if not os.path.exists(LOG_PATH):
        print(f"找不到运行时日志：{LOG_PATH}", file=sys.stderr)
        return 1
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
