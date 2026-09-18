//! Conservative PowerShell recognition for presentation only, never authorization.
//! Ambiguous composition, redirection and error suppression retain failure status.

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
