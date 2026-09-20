//! Conservative PowerShell diagnostics, never authorization or command rewriting.
//! Ambiguous composition, redirection and error suppression retain failure status.

pub(super) const SELECT_COUNT_HINT: &str = "Select-Object 的 -First/-Last/-Skip 必须带行数，例如 `Select-Object -First 20`。命令尚未执行；请补全参数后重新调用，不要重复提交原命令。";
pub(super) const RG_PATH_GLOB_HINT: &str = "PowerShell 不会展开原生 rg 的路径通配符。命令尚未执行；请改用目录和 --glob，例如 `rg -n '标识符' src --glob '*.tsx' --glob '*.css' | Select-Object -First 10`，再重新调用。";
pub(super) const RG_BATCH_HINT: &str = "请把分号串联的多次 rg 搜索拆成独立的 run_command 调用，以便分别判断匹配、未匹配和真实错误。命令尚未执行；不要通过追加 exit 0 或隐藏 stderr 绕过错误。";

/// Reject only literal, recognizable mistakes before starting a process. Unknown
/// shell syntax is left to the shell and the existing authorization policy.
pub(super) fn powershell_preflight_hint(command: &str) -> Option<&'static str> {
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
    let Some(segments) = literal_pipeline(command) else {
        return false;
    };
    let search = &segments[0];
    if !search
        .first()
        .is_some_and(|s| s.eq_ignore_ascii_case("rg") || s.eq_ignore_ascii_case("rg.exe"))
        || search.len() < 2
        || search.iter().any(|arg| {
            arg == "--no-messages"
                || arg == "--quiet"
                || arg.starts_with("--pre")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('q'))
        })
    {
        return false;
    }
    if segments.len() == 1 {
        return true;
    }
    let receiver = &segments[1];
    segments.len() == 2
        && receiver.len() == 3
        && receiver[0].eq_ignore_ascii_case("Select-Object")
        && (receiver[1].eq_ignore_ascii_case("-First") || receiver[1].eq_ignore_ascii_case("-Last"))
        && receiver[2].parse::<usize>().is_ok_and(|count| count > 0)
}

fn literal_pipeline(command: &str) -> Option<Vec<Vec<String>>> {
    let mut segments = vec![Vec::new()];
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    for c in command.chars() {
        if matches!(c, '\r' | '\n' | '`') {
            return None;
        }
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '$' => return None,
            Some(_) => word.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                ';' | '&' | '>' | '<' | '$' | '(' | ')' | '{' | '}' | '#' | ',' => return None,
                '|' => {
                    if started {
                        segments.last_mut()?.push(std::mem::take(&mut word));
                        started = false;
                    }
                    if segments.last()?.is_empty() || segments.len() > 1 {
                        return None;
                    }
                    segments.push(Vec::new());
                }
                c if c.is_whitespace() => {
                    if started {
                        segments.last_mut()?.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                _ => {
                    started = true;
                    word.push(c);
                }
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        segments.last_mut()?.push(word);
    }
    if segments.last()?.is_empty() {
        return None;
    }
    Some(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        ] {
            assert!(!is_unambiguous_rg_search(command), "{command}");
        }
    }
}
