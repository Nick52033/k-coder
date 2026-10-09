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
import re


# ── 分类规则表 ────────────────────────────────────────────────────────────
# 每条规则按顺序匹配，命中即停，所以顺序越靠前越具体。
# verdict 取值：
#   bug      产品缺陷，需要改代码
#   not-bug  按设计 / 模型侧 / 环境侧，无需改代码
#   review   规则没覆盖到，交给人判，不要默认当 bug

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

TEST_NOISE_MARKERS = (
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
    """from_utf8_lossy 对非 UTF-8 字节（中文 Windows 上 PowerShell 的 GBK stderr）
    会写成 U+FFFD；一条输出里出现多个替换符基本可断定是解码问题，不是原文。"""
    return text.count(REPLACEMENT_CHAR) >= 2


def _match(event: str, text: str, *needles: str) -> bool:
    lowered = text.lower()
    return any(needle in event or needle.lower() in lowered for needle in needles)


RULES: list[dict] = [
    {
        "id": "B1",
        "verdict": "bug",
        "title": "移动网关恢复失败：绑定地址失效后既不回退也不提示",
        "detail": "restore_on_startup 只在 bind 失败时写日志；resolve_bind_ip 只校验"
                  " IP 字面量是否属于回环/私网，不校验是否真的是本机网卡地址，"
                  "于是失效地址（如 WSL 虚拟网卡）能过校验但必然 bind 失败，"
                  "用户侧只看到「未运行」，拿不到原因。",
        "evidence": "src-tauri/src/mobile/mod.rs:192、src-tauri/src/mobile/server.rs:172",
        "fix": "start() bind 失败时回退 loopback 并持久化；"
               "或把失败原因透出到 MobileStatus 供设置页展示。",
        "match": lambda event, text: "mobile.gateway.restore_failed" in event,
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
        "match": lambda event, text: _match(event, text, "is not enabled", "plugin"),
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
        "verdict": "not-bug",
        "title": "Provider 返回 HTTP 错误",
        "detail": "账号实名认证、配额、限流或服务端故障，属供应商侧；换模型或供应商即可。",
        "evidence": "fields.status / retryAfterMs 已随记录落库",
        "match": lambda event, text: _match(event, text, "provider returned http"),
    },
    {
        "id": "N7",
        "verdict": "not-bug",
        "title": "模型空响应（completed 但既无文本也无工具调用）",
        "detail": "模型把输出预算几乎全用于 reasoning 后空完成。源码已有空响应守卫，"
                  "若部署 exe 早于源码改动则为部署滞后，不是代码缺陷。",
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
        "title": "命令自身失败：无匹配、路径不存在、正则不合法",
        "detail": "rg 退出码 1（无命中）、模型套用了别的仓库目录、正则写错，均属模型侧命令问题。",
        "evidence": "run_command 退出码与 stderr",
        "match": lambda event, text: _match(
            event, text,
            "no matches (exit code 1)",
            "command produced no output and exited with code",
            "系统找不到指定的文件",
            "未能找到路径",
            "regex parse error",
            "unclosed character class",
        ),
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
        "verdict": "not-bug",
        "title": "测试运行自身的输出与退出码",
        "detail": "Playwright / vite / pnpm 的 stderr 与断言失败被当成 error 记录，属测试侧噪声。",
        "evidence": "run_command output 片段",
        "match": lambda event, text: any(marker in text for marker in TEST_NOISE_MARKERS),
    },
    {
        "id": "N14",
        "verdict": "not-bug",
        "title": "命令有输出但退出码非零（PowerShell 管道/串联/重定向造成的伪失败）",
        "detail": "stdout 已经拿到想要的内容，退出码却非零：PowerShell 里 `2>&1` 会把原生命令的 stderr "
                  "并进输出流并记一条 NativeCommandError，`;` 串联后以最后一条的退出码收尾，"
                  "`| Select-Object -First N` 提前收手还会掐断上游进程。"
                  "k-coder 有意只对「无错误抑制、形状无歧义的 rg … | Select-Object -First N」判成功，"
                  "带重定向、错误抑制、串联的一律保持失败，否则模型永远学不到自己的命令写错了。",
        "evidence": "src-tauri/src/tools/command_diagnostics.rs is_bounded_rg_search / is_unambiguous_rg_search",
        "fix": "无需改代码。模型侧应拆成单条命令、去掉 2>&1 与 `;` 串联，或改用 Read/Grep 类工具。",
        "match_record": lambda record: is_nonzero_exit_with_output(record),
    },
    {
        "id": "N15",
        "verdict": "not-bug",
        "title": "模型把 bash heredoc 带到 PowerShell（python - <<'PY'）",
        "detail": "PowerShell 没有 heredoc 语法，命令在解析期就失败。属模型侧命令问题；"
                  "这类记录的 stderr 是中文报错，常同时命中 B2 解码缺陷，但主因仍是命令写错。",
        "evidence": "fields.arguments.command 含 <<；stderr 为 PowerShell ParserError",
        "fix": "无需改代码。模型侧改用 WriteFile 工具或单行 python -c。",
        "match_record": lambda record: (
            record.get("event") == "tool_failed"
            and "<<" in str((record.get("fields", {}).get("arguments") or {}).get("command") or "")
        ),
    },
]


# ── 伴随缺陷（不决定那次调用为什么失败，但确实是产品问题）────────────────────
# 这些规则单独成表：它们命中的记录已有主因，若并进主表会把「模型命令写错」
# 误报成「解码 bug 导致的失败」，两头都对不上。
OBSERVATIONS: list[dict] = [
    {
        "id": "B2",
        "verdict": "bug",
        "title": "PowerShell 中文报错被按 UTF-8 解码，原文变成乱码",
        "detail": "execution.rs 用 String::from_utf8_lossy 直接解码子进程输出，未回退到系统 ANSI 代码页"
                  "（中文 Windows 为 CP936/GBK），于是 PowerShell 的原生报错读不出来，"
                  "模型和人都看不出这次失败的真实原因。",
        "evidence": "src-tauri/src/execution.rs:853、src-tauri/src/execution.rs:1439",
        "fix": "解码失败时按当前 ANSI 代码页重试，或让 PowerShell 以 UTF-8 输出。",
        "match": lambda event, text: has_replacement_chars(text),
    },
]

UNMATCHED: dict = {
    "id": "R0",
    "verdict": "review",
    "title": "规则未覆盖，需人工判定",
    "detail": "新出现的错误形态。先按 errors / session 子命令复现，再决定是补规则还是修代码。",
    "evidence": "",
    "fix": "补规则或修代码，二选一，别把未判定项直接算成 bug。",
}

# 会让原生命令退出码偏离「命令本身成功与否」的 PowerShell 写法。
EXIT_CODE_SHIFTING = ("2>&1", "2>$null", ";", "|", "&&", "||", ">", "-ErrorAction", "$(", "if (")


def is_nonzero_exit_with_output(record: dict) -> bool:
    """run_command 拿到了输出却仍被判 failed：退出码被管道/串联/重定向带偏。"""
    if record.get("event") != "tool_failed":
        return False
    fields = record.get("fields", {})
    command = (fields.get("arguments") or {}).get("command")
    if not command or not fields.get("output"):
        return False
    return any(token in command for token in EXIT_CODE_SHIFTING)


VERDICT_LABEL = {
    "bug": "需要修复（产品缺陷）",
    "review": "待人工判定",
    "not-bug": "不是 bug（按设计 / 模型侧 / 环境侧）",
}


def record_text(record: dict) -> str:
    fields = record.get("fields", {})
    parts = [
        str(fields.get("message") or ""),
        str(fields.get("output") or ""),
        str(fields.get("reason") or ""),
        str(fields.get("errorCode") or ""),
        str(fields.get("tool") or ""),
    ]
    arguments = fields.get("arguments")
    if isinstance(arguments, dict):
        # 命令原文也要参与匹配：PowerShell 对 heredoc、bash 管道的报错是中文，
        # 被 B2 解码问题搅成乱码后标记匹配不上，只能靠命令本身识别。
        parts.append(str(arguments.get("command") or ""))
    return "\n".join(parts)


def classify_record_with_observations(record: dict) -> tuple[dict, list[dict]]:
    """返回 (主因规则, 伴随缺陷规则列表)。主因解释这次调用为什么失败，
    伴随缺陷是同一批记录里另有的产品问题，两者不相加。"""
    event = record.get("event", "")
    text = record_text(record)
    primary = UNMATCHED
    for rule in RULES:
        if rule.get("match_record") is not None:
            if rule["match_record"](record):
                primary = rule
                break
        elif rule["match"](event, text):
            primary = rule
            break
    extras = [rule for rule in OBSERVATIONS if rule["match"](event, text)]
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
    text = text.replace("\r\n", "\\n").replace("\n", "\\n")
    return text if len(text) <= limit else text[: limit - 1] + "…"


def print_group(group: dict, verbose: bool = False) -> None:
    rule = group["rule"]
    records = group["records"]
    times = [r.get("timestampMs", 0) for r in records]
    threads: list[str] = []
    for record in records:
        thread = record.get("fields", {}).get("threadId") or "(无 threadId)"
        if thread not in threads:
            threads.append(thread)
    tag = "（伴随缺陷，不决定上面那些条目的主因）" if group.get("cooccurring") else ""
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
            fields = record.get("fields", {})
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
    rest_n = sum(1 for _, rule, extras in classified
                 if rule["verdict"] == "not-bug" and not extras)
    print(f"\n{'━' * 70}")
    print("结论")
    print("━" * 70)
    if primary_bugs:
        names = "、".join(f"{g['rule']['id']}×{len(g['records'])}" for g in primary_bugs)
        print(f"· 需要修复：{primary_bug_n} 条，主因是产品缺陷（{names}）。")
    else:
        print("· 没有需要修复的产品缺陷。")
    if observations:
        names = "、".join(f"{g['rule']['id']}×{len(g['records'])}" for g in observations.values())
        print(f"· 伴随缺陷：{obs_n} 条（{names}）——这些记录另有主因，但报错正文不可读这点要修。")
    if review_n:
        print(f"· 待人工判定：{review_n} 条规则没覆盖，先跑 errors / session 复现再定性。")
    print(f"· 其余 {rest_n} 条为按设计行为或模型侧/环境侧问题，"
          "无需改代码；不要把它包装成待修问题。")
    if primary_bugs or observations:
        pointers = "、".join(
            g["rule"]["evidence"] for g in bugs if g["rule"].get("evidence"))
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
        thread = record.get("fields", {}).get("threadId") or "(无 threadId)"
        by_thread.setdefault(thread, []).append(record)

    for thread, records in sorted(by_thread.items(), key=lambda kv: kv[1][0]["timestampMs"]):
        session_file = os.path.join(SESSION_DIR, f"{thread}.jsonl")
        last_active = ""
        if os.path.exists(session_file):
            last_ms = 0
            with open(session_file, encoding="utf-8") as handle:
                for line in handle:
                    try:
                        ms = json.loads(line).get("createdAtMs", 0)
                    except json.JSONDecodeError:
                        continue
                    last_ms = max(last_ms, ms)
            last_active = f"  会话最近活跃 {fmt_time(last_ms)}"
        print(f"\n== thread {thread}  ({len(records)} 条 error){last_active}")
        for record in records:
            fields = record.get("fields", {})
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
            ms = record.get("createdAtMs", 0)
            if args.since_ms and ms < args.since_ms:
                continue
            event = record.get("type")
            if event not in SESSION_EVENTS:
                continue
            data = record.get("data", {})
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
    if event == "user_message":
        message = data.get("message", {})
        text = " ".join(part.get("text", "")
                        for part in message.get("content", []) if part.get("type") == "text")
        return f"{stamp}  USER: {clip(text, 300)}"
    if event == "thread_model_selected":
        return f"{stamp}  MODEL: {data.get('model')}  (provider={data.get('provider_id')})"
    if event == "provider_call_usage":
        usage = data.get("usage", {})
        details = data.get("details", {})
        return (f"{stamp}  USAGE #{data.get('call_index')}  {data.get('model')}"
                f"  出={usage.get('outputTokens')}"
                f"  思考={details.get('reasoningOutputTokens')}")
    if event == "assistant_tool_calls":
        calls = data.get("calls", [])
        parts = []
        for call in calls:
            arguments = call.get("arguments", {})
            brief = arguments.get("command") or arguments.get("path") or json.dumps(
                arguments, ensure_ascii=False)
            parts.append(f"{call.get('name')}({clip(brief, 160)})")
        return f"{stamp}  CALL: " + " | ".join(parts)
    if event == "tool_result":
        result = data.get("result", {})
        meta = result.get("metadata", {})
        return (f"{stamp}  RESULT {data.get('name')} {data.get('call_id')}"
                f"  success={result.get('success')} exit={meta.get('exitCode')}"
                f"  out={clip(result.get('output') or '', 160)}")
    if event == "assistant_message":
        message = data.get("message", {})
        text = " ".join(part.get("text", "")
                        for part in message.get("content", []) if part.get("type") == "text")
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
