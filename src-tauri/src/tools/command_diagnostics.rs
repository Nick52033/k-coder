//! Conservative PowerShell diagnostics, never authorization or command rewriting.
//! Ambiguous composition, redirection and error suppression retain failure status.

pub(super) const SELECT_COUNT_HINT: &str = "Select-Object 的 -First/-Last/-Skip 必须带行数，例如 `Select-Object -First 20`。命令尚未执行；请补全参数后重新调用，不要重复提交原命令。";
pub(super) const RG_PATH_GLOB_HINT: &str = "PowerShell 不会展开原生 rg 的路径通配符。命令尚未执行；请改用目录和 --glob，例如 `rg -n '标识符' src --glob '*.tsx' --glob '*.css' | Select-Object -First 10`，再重新调用。";
pub(super) const PYTHON_HEREDOC_HINT: &str = "PowerShell 不支持 Bash 的 python - <<'PY' heredoc。命令尚未执行；多行 Python 请写入脚本文件后运行，单行使用 python -c，不要重复提交原命令。";
pub(super) const RG_BATCH_HINT: &str = "请把分号串联的多次 rg 搜索拆成独立的 run_command 调用，以便分别判断匹配、未匹配和真实错误。命令尚未执行；不要通过追加 exit 0 或隐藏 stderr 绕过错误。";
pub(super) const NATIVE_STDERR_HINT: &str = "PowerShell 将原生 stderr 呈现为错误记录，不能仅凭有输出判定成功。构建或测试请直接运行，若命令含 2>&1 或截取管道请去掉；运行时已限制输出大小。核对完整失败汇总并保留真实退出码，不要追加 exit 0，也不要原样重试有副作用的命令。";

pub(super) fn powershell_native_stderr_hint(stderr: &str) -> Option<&'static str> {
    stderr
        .contains("NativeCommandError")
        .then_some(NATIVE_STDERR_HINT)
}

/// Reject only literal, recognizable mistakes before starting a process. Unknown
/// shell syntax is left to the shell and the existing authorization policy.
pub(super) fn powershell_preflight_hint(command: &str) -> Option<&'static str> {
    let mut words = command.split_whitespace();
    if words.next().is_some_and(|word| {
        ["python", "python3", "py"]
            .iter()
            .any(|name| word.eq_ignore_ascii_case(name))
    }) && words.next() == Some("-")
        && words.next().is_some_and(|word| word.starts_with("<<"))
    {
        return Some(PYTHON_HEREDOC_HINT);
    }
    let script = literal_script(command)?;
    for pipeline in &script {
        for segment in pipeline {
            if is_select_object(segment) {
                for (index, word) in segment.iter().enumerate().skip(1) {
                    if word.bare && word.value == "--" {
                        break;
                    }
                    if word.bare
                        && ["-First", "-Last", "-Skip"]
                            .iter()
                            .any(|parameter| word.value.eq_ignore_ascii_case(parameter))
                        && segment
                            .get(index + 1)
                            .is_none_or(|next| next.bare && next.value.starts_with('-'))
                    {
                        return Some(SELECT_COUNT_HINT);
                    }
                }
            }
        }
    }
    for pipeline in &script {
        for segment in pipeline {
            if let Some(paths) = rg_paths(segment)
                && paths.iter().any(|path| path.contains(['*', '?']))
            {
                return Some(RG_PATH_GLOB_HINT);
            }
        }
    }
    if script.len() > 1
        && script.iter().all(|pipeline| {
            rg_paths(&pipeline[0]).is_some()
                && (pipeline.len() == 1 || (pipeline.len() == 2 && is_select_object(&pipeline[1])))
        })
    {
        return Some(RG_BATCH_HINT);
    }
    None
}

struct LiteralWord {
    value: String,
    bare: bool,
}

fn is_select_object(segment: &[LiteralWord]) -> bool {
    segment.first().is_some_and(|word| {
        word.bare
            && (word.value.eq_ignore_ascii_case("Select-Object")
                || word.value.eq_ignore_ascii_case("select"))
    })
}

/// Parse only known rg options so regexes and option values are never mistaken
/// for paths. This allowlist is diagnostic, not a permission allowlist.
fn rg_paths(segment: &[LiteralWord]) -> Option<Vec<&str>> {
    let executable = segment.first()?;
    if !executable.bare
        || !(executable.value.eq_ignore_ascii_case("rg")
            || executable.value.eq_ignore_ascii_case("rg.exe"))
    {
        return None;
    }
    let mut pattern_supplied = false;
    let mut positional = Vec::new();
    let mut options = true;
    let mut words = segment.iter().skip(1);
    while let Some(word) = words.next() {
        let value = word.value.as_str();
        if options && value == "--" {
            options = false;
        } else if options && value.starts_with("--") {
            let (option, inline) = value
                .split_once('=')
                .map_or((value, None), |(name, value)| (name, Some(value)));
            match option {
                "--regexp" | "--file" | "--glob" | "--iglob" | "--type" | "--type-not"
                | "--after-context" | "--before-context" | "--context" | "--max-count"
                | "--max-depth" | "--encoding" | "--color" => {
                    inline.or_else(|| words.next().map(|word| word.value.as_str()))?;
                    pattern_supplied |= matches!(option, "--regexp" | "--file");
                }
                "--files" if inline.is_none() => pattern_supplied = true,
                "--line-number"
                | "--ignore-case"
                | "--smart-case"
                | "--case-sensitive"
                | "--fixed-strings"
                | "--word-regexp"
                | "--line-regexp"
                | "--files-with-matches"
                | "--files-without-match"
                | "--hidden"
                | "--no-ignore"
                | "--no-ignore-vcs"
                | "--crlf"
                | "--only-matching"
                | "--column"
                | "--heading"
                | "--no-heading"
                | "--count"
                | "--count-matches"
                | "--with-filename"
                | "--no-filename"
                | "--multiline"
                | "--multiline-dotall"
                | "--pcre2"
                | "--json"
                | "--stats"
                    if inline.is_none() => {}
                _ => return None,
            }
        } else if options && value.starts_with('-') && value != "-" {
            let mut flags = value[1..].char_indices();
            while let Some((index, flag)) = flags.next() {
                match flag {
                    'e' | 'f' | 'g' | 't' | 'T' | 'A' | 'B' | 'C' | 'm' | 'E' => {
                        if index + flag.len_utf8() == value.len() - 1 {
                            words.next()?;
                        }
                        pattern_supplied |= matches!(flag, 'e' | 'f');
                        break;
                    }
                    'n' | 'i' | 'S' | 's' | 'F' | 'w' | 'x' | 'l' | 'o' | 'c' | 'H' | 'I' | 'U'
                    | 'P' | 'v' | 'a' => {}
                    _ => return None,
                }
            }
        } else {
            positional.push(value);
        }
    }
    if !pattern_supplied {
        if positional.is_empty() {
            return None;
        }
        positional.remove(0);
    }
    Some(positional)
}

/// A deliberately small literal subset, with quote boundaries retained for
/// PowerShell command names and named parameters. No expansion or evaluation.
fn literal_script(command: &str) -> Option<Vec<Vec<Vec<LiteralWord>>>> {
    let mut script = vec![vec![Vec::new()]];
    let mut value = String::new();
    let mut started = false;
    let mut bare = true;
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if matches!(c, '\r' | '\n' | '`') {
            return None;
        }
        match quote {
            Some(q) if c == q => {
                if chars.peek() == Some(&q) {
                    value.push(q);
                    chars.next();
                } else {
                    quote = None;
                }
            }
            Some('"') if c == '$' => return None,
            Some(_) => value.push(c),
            None => match c {
                '\'' | '"' if !started => {
                    quote = Some(c);
                    started = true;
                    bare = false;
                }
                '\'' | '"' | '&' | '>' | '<' | '$' | '@' | '(' | ')' | '{' | '}' | '#' | ',' => {
                    return None;
                }
                c if c.is_whitespace() || matches!(c, '|' | ';') => {
                    let pipeline = script.last_mut()?;
                    if started {
                        pipeline.last_mut()?.push(LiteralWord {
                            value: std::mem::take(&mut value),
                            bare,
                        });
                        started = false;
                        bare = true;
                    }
                    if matches!(c, '|' | ';') {
                        if pipeline.last()?.is_empty() {
                            return None;
                        }
                        if c == '|' {
                            pipeline.push(Vec::new());
                        } else {
                            script.push(vec![Vec::new()]);
                        }
                    }
                }
                _ if !bare => return None,
                _ => {
                    value.push(c);
                    started = true;
                }
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        script
            .last_mut()?
            .last_mut()?
            .push(LiteralWord { value, bare });
    }
    if script.last()?.last()?.is_empty() {
        return None;
    }
    if script
        .iter()
        .flatten()
        .flatten()
        .any(|word| word.bare && word.value == "--%")
    {
        return None;
    }
    Some(script)
}

pub(super) fn is_unambiguous_rg_search(command: &str) -> bool {
    let Some(script) = literal_script(command) else {
        return false;
    };
    if script.len() != 1 {
        return false;
    }
    let segments = &script[0];
    if !is_plain_rg_search(&segments[0]) {
        return false;
    }
    if segments.len() == 1 {
        return true;
    }
    let receiver = &segments[1];
    segments.len() == 2
        && receiver.len() == 3
        && receiver[0].bare
        && receiver[0].value.eq_ignore_ascii_case("Select-Object")
        && receiver[1].bare
        && (receiver[1].value.eq_ignore_ascii_case("-First")
            || receiver[1].value.eq_ignore_ascii_case("-Last"))
        && receiver[2]
            .value
            .parse::<usize>()
            .is_ok_and(|count| count > 0)
}

/// Detect a native `rg` search whose bounded `Select-Object -First` receiver
/// stops reading before rg finishes writing. On Windows PowerShell this leaves
/// rg with a broken pipe and exit code 1 even though the delivered lines are
/// complete and correct, so the non-zero exit code is an artifact of the
/// truncation rather than a search failure. Only the literal, unambiguous
/// `rg ... | Select-Object -First N` shape qualifies: dynamic expressions,
/// redirection and error suppression keep the original failure status.
pub(super) fn is_bounded_rg_search(command: &str) -> bool {
    let Some(script) = literal_script(command) else {
        return false;
    };
    if script.len() != 1 {
        return false;
    }
    let segments = &script[0];
    segments.len() == 2 && is_plain_rg_search(&segments[0]) && is_first_bounded_select(&segments[1])
}

/// A single rg invocation without error suppression, so an exit code still
/// reflects the search itself instead of an explicitly hidden failure.
fn is_plain_rg_search(search: &[LiteralWord]) -> bool {
    rg_paths(search).is_some()
}

fn is_first_bounded_select(receiver: &[LiteralWord]) -> bool {
    receiver.len() == 3
        && receiver[0].bare
        && receiver[0].value.eq_ignore_ascii_case("Select-Object")
        && receiver[1].bare
        && receiver[1].value.eq_ignore_ascii_case("-First")
        && receiver[2]
            .value
            .parse::<usize>()
            .is_ok_and(|count| count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_rejects_only_recognizable_python_heredocs() {
        for command in [
            "python - <<'PY'\nprint(1)\nPY",
            "py - <<EOF",
            "python3 - <<'END'",
        ] {
            assert_eq!(
                powershell_preflight_hint(command),
                Some(PYTHON_HEREDOC_HINT)
            );
        }
        for command in [
            "python -c \"print(1)\"",
            "python script.py",
            "Write-Output 'python - <<PY'",
            "python - $args",
        ] {
            assert_eq!(powershell_preflight_hint(command), None);
        }
    }

    #[test]
    fn preflight_catches_missing_select_counts_before_any_command_runs() {
        for command in [
            "rg -n 'AgentActivityStatus' src/App.tsx | Select-Object -First",
            "Get-Content source.txt | select -last",
            "Select-Object -Skip -First 20",
            "rg x src | Select-Object -First; rg y src",
            "Set-Content marker.txt changed | Select-Object -First",
        ] {
            assert_eq!(
                powershell_preflight_hint(command),
                Some(SELECT_COUNT_HINT),
                "{command}"
            );
        }
    }

    #[test]
    fn preflight_recognizes_rg_paths_without_confusing_patterns_or_option_values() {
        for command in [
            "rg -n 'ModeSelector.css|composer-popover-surface' src/*.tsx src/**/*.tsx src/**/*.css | Select-Object -First 10",
            "rg -n needle src/App.tsx src/**/*.css",
            "rg.exe --files 'src dir/*.tsx'",
            "rg -n -A 10 -e 'interface.*ActivityStatus' src/*.ts",
            "rg -niA10 -e 'x/y.*' src/*.ts",
            "rg --regexp=x/y.* src/*.ts",
            "rg -- -pattern *.ts",
        ] {
            assert_eq!(
                powershell_preflight_hint(command),
                Some(RG_PATH_GLOB_HINT),
                "{command}"
            );
        }
    }

    #[test]
    fn preflight_requests_separate_searches_instead_of_guessing_aggregate_exit_status() {
        let command = "rg -n 'activityStatus' src/App.tsx src/stores/workbenchStore.ts | Select-Object -First 20; rg -n 'ActivityStatusView|interface.*ActivityStatus' -A 10 src/types/runtime.ts | Select-Object -First 40";
        assert_eq!(powershell_preflight_hint(command), Some(RG_BATCH_HINT));
        assert!(!is_unambiguous_rg_search(command));
    }

    #[test]
    fn preflight_leaves_valid_and_ambiguous_scripts_unchanged() {
        for command in [
            "rg -n 'AgentActivityStatus' src/App.tsx | Select-Object -First 20",
            "rg -n 'x/y.*' src --glob '*.tsx' --glob '*.css'",
            "rg --glob 'src/**/*.tsx' 'x/y.*' .",
            "rg -gsrc/*.tsx -n 'a;b|x/y.*' .",
            "rg --regexp 'x/y.*' --glob '*.ts' src",
            "rg --regexp='x/y.*' src/*.ts", // mixed quoting is left to the shell
            "rg --files src | Select-Object -First 0",
            "rg 'don''t/match.*' src",
            "rg --unknown-option 'src/*.tsx' .",
            "rg --pre processor needle src/*.tsx",
            "rg x .; exit 1",
            "rg x . 2>$null",
            "rg x $paths | Select-Object -First $limit",
            "Write-Output 'Select-Object -First'",
            "Write-Output 'Select-Object' '-First'",
            "Get-Process | Select-Object '-First'",
            "Get-Process | Select-Object -- -First",
            "rg --% x | Select-Object -First",
            "Select-Object @options -First",
            "'Select-Object' -First",
            "rg \"unclosed ./sample*.cs",
            "rg x src; Write-Output done",
            "rg x src\nWrite-Output done",
            "rg x src && rg y src",
        ] {
            assert_eq!(powershell_preflight_hint(command), None, "{command}");
        }
    }

    #[test]
    fn recognizes_literal_searches_and_a_single_output_limit() {
        for command in [
            "rg missing .",
            "rg.exe -n 'first|second' --glob '*.cs' src | Select-Object -First 20",
            "rg -n \"x\" . | select-object -last 10",
            "rg --files src",
            "rg -equery .",
            "rg 'don''t/match.*' src",
        ] {
            assert!(is_unambiguous_rg_search(command), "{command}");
        }
    }

    #[test]
    fn never_labels_ambiguous_or_suppressed_failures_as_no_matches() {
        for command in [
            "echo rg; exit 1",
            "rg x .; exit 1",
            "rg x missing 2>$null",
            "rg --no-messages x missing",
            "rg -nq x missing",
            "rg --quiet x missing",
            "rg --pre bad x .",
            "rg x . | rg y",
            "rg x . | Select-Object -First 0",
            "rg x . | Select-Object -First 2; exit 1",
            "rg x . | Select-Object -First 2 | bad",
            "rg x . | bad",
            "rg x . #comment",
            "rg x .\nexit 1",
            "rg x $path",
            "rg x $(bad)",
            "rg x . >out",
            "rg 'unterminated",
            "'rg' x .",
            "rg x . | 'Select-Object' -First 2",
            "rg x . | Select-Object '-First' 2",
            "rg x @paths",
            "rg --% x .",
            "rg --unknown-option x .",
        ] {
            assert!(!is_unambiguous_rg_search(command), "{command}");
        }
    }

    #[test]
    fn bounded_searches_preserve_literal_command_and_parameter_boundaries() {
        assert!(is_bounded_rg_search(
            "rg -equery . | Select-Object -First 2"
        ));
        for command in [
            "'rg' x . | Select-Object -First 2",
            "rg x . | 'Select-Object' -First 2",
            "rg x . | Select-Object '-First' 2",
            "rg x @paths | Select-Object -First 2",
            "rg --% x . | Select-Object -First 2",
            "rg --quiet x . | Select-Object -First 2",
            "rg --no-messages x . | Select-Object -First 2",
            "rg --unknown-option x . | Select-Object -First 2",
            "rg x . | Select-Object -Last 2",
            "rg x . | Select-Object -First 2; exit 1",
        ] {
            assert!(!is_bounded_rg_search(command), "{command}");
        }
    }

    #[test]
    fn native_stderr_guidance_does_not_claim_success_or_replay_commands() {
        assert_eq!(
            powershell_native_stderr_hint("CategoryInfo: NotSpecified\nNativeCommandError"),
            Some(NATIVE_STDERR_HINT)
        );
        for stderr in ["", "Finished test profile", "rg: regex parse error"] {
            assert_eq!(powershell_native_stderr_hint(stderr), None);
        }
        assert!(NATIVE_STDERR_HINT.contains("保留真实退出码"));
        assert!(NATIVE_STDERR_HINT.contains("不要原样重试有副作用"));
    }
}
