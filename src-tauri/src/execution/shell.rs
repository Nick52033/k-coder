use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use super::{CommandAssessment, CommandMode, CommandRisk, StartCommandRequest, assess_command};

const POWERSHELL_UTF8_OUTPUT_PREFIX: &str =
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";
#[cfg(windows)]
const POWERSHELL_UTF8_INTERACTIVE_INIT: &str = "try { [Console]::InputEncoding=[System.Text.Encoding]::UTF8; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellType {
    #[cfg(not(windows))]
    Zsh,
    #[cfg(not(windows))]
    Bash,
    #[cfg(windows)]
    PowerShell,
    #[cfg(not(windows))]
    Sh,
    #[cfg(windows)]
    Cmd,
}

impl ShellType {
    fn name(self) -> &'static str {
        match self {
            #[cfg(not(windows))]
            Self::Zsh => "zsh",
            #[cfg(not(windows))]
            Self::Bash => "bash",
            #[cfg(windows)]
            Self::PowerShell => "powershell",
            #[cfg(not(windows))]
            Self::Sh => "sh",
            #[cfg(windows)]
            Self::Cmd => "cmd",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct DetectedShell {
    shell_type: ShellType,
    program: PathBuf,
}

impl DetectedShell {
    pub(super) fn name(&self) -> &'static str {
        self.shell_type.name()
    }

    pub(super) fn uses_windows_powershell_native_pipeline(&self) -> bool {
        #[cfg(windows)]
        {
            self.shell_type == ShellType::PowerShell
                && self
                    .program
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("powershell"))
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    pub(super) fn request(
        &self,
        command: &str,
        cwd: String,
        timeout_ms: u64,
    ) -> StartCommandRequest {
        let program = self.program.to_string_lossy().into_owned();
        let args = match self.shell_type {
            #[cfg(not(windows))]
            ShellType::Zsh | ShellType::Bash | ShellType::Sh => {
                vec!["-c".to_string(), command.to_string()]
            }
            #[cfg(windows)]
            ShellType::PowerShell => vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("{POWERSHELL_UTF8_OUTPUT_PREFIX}{command}"),
            ],
            #[cfg(windows)]
            ShellType::Cmd => vec!["/c".to_string(), command.to_string()],
        };
        StartCommandRequest {
            program,
            args,
            cwd,
            env: HashMap::new(),
            mode: CommandMode::Foreground,
            timeout_ms: Some(timeout_ms),
            buffer_bytes: None,
        }
    }

    pub(super) fn assess(&self, command: &str) -> CommandAssessment {
        assess_script(self.shell_type, command)
    }

    /// 交互式终端（PTY）启动参数：程序路径与保持会话存活的初始化参数。
    pub(super) fn interactive_launch(&self) -> (String, Vec<String>) {
        let program = self.program.to_string_lossy().into_owned();
        let args = match self.shell_type {
            #[cfg(windows)]
            ShellType::PowerShell => vec![
                "-NoProfile".to_string(),
                "-NoExit".to_string(),
                "-Command".to_string(),
                POWERSHELL_UTF8_INTERACTIVE_INIT.to_string(),
            ],
            #[cfg(windows)]
            ShellType::Cmd => vec![
                "/D".to_string(),
                "/K".to_string(),
                "chcp 65001>nul".to_string(),
            ],
            #[cfg(not(windows))]
            ShellType::Zsh | ShellType::Bash | ShellType::Sh => Vec::new(),
        };
        (program, args)
    }
}

pub(super) fn default_user_shell() -> DetectedShell {
    default_user_shell_for_platform()
}

#[cfg(windows)]
fn default_user_shell_for_platform() -> DetectedShell {
    if let Some(program) = find_executable("pwsh.exe")
        .or_else(|| existing_file(Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe")))
    {
        return DetectedShell {
            shell_type: ShellType::PowerShell,
            program,
        };
    }
    if let Some(program) = find_executable("powershell.exe").or_else(|| {
        existing_file(Path::new(
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ))
    }) {
        return DetectedShell {
            shell_type: ShellType::PowerShell,
            program,
        };
    }
    DetectedShell {
        shell_type: ShellType::Cmd,
        program: PathBuf::from("cmd.exe"),
    }
}

#[cfg(not(windows))]
fn default_user_shell_for_platform() -> DetectedShell {
    if let Some(shell) = env::var_os("SHELL")
        .map(PathBuf::from)
        .and_then(detect_available_shell)
    {
        return shell;
    }

    #[cfg(target_os = "macos")]
    let candidates = [ShellType::Zsh, ShellType::Bash];
    #[cfg(not(target_os = "macos"))]
    let candidates = [ShellType::Bash, ShellType::Zsh];

    for shell_type in candidates {
        if let Some(shell) = find_shell(shell_type) {
            return shell;
        }
    }
    DetectedShell {
        shell_type: ShellType::Sh,
        program: existing_file(Path::new("/bin/sh")).unwrap_or_else(|| PathBuf::from("sh")),
    }
}

#[cfg(not(windows))]
fn detect_available_shell(path: PathBuf) -> Option<DetectedShell> {
    let shell_type = detect_shell_type(&path)?;
    let program = if path.components().count() > 1 {
        existing_file(&path)?
    } else {
        find_executable(path.to_str()?)?
    };
    Some(DetectedShell {
        shell_type,
        program,
    })
}

#[cfg(not(windows))]
fn find_shell(shell_type: ShellType) -> Option<DetectedShell> {
    let (name, fallback_paths): (&str, &[&str]) = match shell_type {
        ShellType::Zsh => ("zsh", &["/bin/zsh"]),
        ShellType::Bash => ("bash", &["/bin/bash", "/usr/bin/bash"]),
        ShellType::Sh => ("sh", &["/bin/sh"]),
        ShellType::PowerShell | ShellType::Cmd => return None,
    };
    let program = find_executable(name).or_else(|| {
        fallback_paths
            .iter()
            .find_map(|path| existing_file(Path::new(path)))
    })?;
    Some(DetectedShell {
        shell_type,
        program,
    })
}

#[cfg(not(windows))]
fn detect_shell_type(path: &Path) -> Option<ShellType> {
    match path
        .file_stem()
        .and_then(|name| name.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "zsh" => Some(ShellType::Zsh),
        "bash" => Some(ShellType::Bash),
        "sh" => Some(ShellType::Sh),
        _ => None,
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return existing_file(path);
    }
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths).find_map(|directory| existing_file(&directory.join(name)))
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

fn assess_script(shell_type: ShellType, command: &str) -> CommandAssessment {
    let Some(parts) = parse_simple_command(shell_type, command) else {
        return CommandAssessment {
            risk: CommandRisk::Write,
            requires_approval: true,
            reason: "shell composition or dynamic syntax requires approval because its effects cannot be proven read-only".into(),
        };
    };
    let Some((program, args)) = parts.split_first() else {
        return CommandAssessment {
            risk: CommandRisk::Write,
            requires_approval: true,
            reason: "empty shell commands are not allowed".into(),
        };
    };
    assess_command(program, args)
}

fn parse_simple_command(_shell_type: ShellType, command: &str) -> Option<Vec<String>> {
    #[derive(Clone, Copy)]
    enum Quote {
        Single,
        Double,
    }

    let mut quote = None;
    let mut current = String::new();
    let mut parts = Vec::new();
    for character in command.chars() {
        match quote {
            Some(Quote::Single) => {
                if character == '\'' {
                    quote = None;
                } else if matches!(character, '\r' | '\n') {
                    return None;
                } else {
                    current.push(character);
                }
            }
            Some(Quote::Double) => {
                if character == '"' {
                    quote = None;
                } else if matches!(character, '$' | '`' | '\r' | '\n') {
                    return None;
                } else {
                    current.push(character);
                }
            }
            None => match character {
                '\'' => quote = Some(Quote::Single),
                '"' => quote = Some(Quote::Double),
                character if character.is_whitespace() => {
                    if !current.is_empty() {
                        parts.push(std::mem::take(&mut current));
                    }
                }
                ';' | '|' | '&' | '>' | '<' | '`' | '$' | '(' | ')' | '{' | '}' => {
                    return None;
                }
                #[cfg(windows)]
                '%' | '!' if _shell_type == ShellType::Cmd => return None,
                _ => current.push(character),
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform_shell_type() -> ShellType {
        #[cfg(windows)]
        {
            ShellType::PowerShell
        }
        #[cfg(not(windows))]
        {
            ShellType::Bash
        }
    }

    #[test]
    fn derives_codex_style_shell_arguments() {
        #[cfg(windows)]
        {
            let powershell = DetectedShell {
                shell_type: ShellType::PowerShell,
                program: PathBuf::from("pwsh.exe"),
            };
            let request = powershell.request("Write-Output 'hello'", ".".into(), 1_000);
            assert_eq!(request.program, "pwsh.exe");
            assert_eq!(&request.args[..2], ["-NoProfile", "-Command"]);
            assert_eq!(
                request.args[2],
                format!("{POWERSHELL_UTF8_OUTPUT_PREFIX}Write-Output 'hello'")
            );

            let cmd = DetectedShell {
                shell_type: ShellType::Cmd,
                program: PathBuf::from("cmd.exe"),
            };
            assert_eq!(
                cmd.request("echo hello", "".into(), 1_000).args,
                ["/c", "echo hello"]
            );
        }

        #[cfg(not(windows))]
        {
            let bash = DetectedShell {
                shell_type: ShellType::Bash,
                program: PathBuf::from("/bin/bash"),
            };
            assert_eq!(
                bash.request("printf hello", "".into(), 1_000).args,
                ["-c", "printf hello"]
            );
        }
    }

    #[test]
    fn distinguishes_windows_powershell_from_pwsh_native_pipelines() {
        #[cfg(windows)]
        {
            let windows_powershell = DetectedShell {
                shell_type: ShellType::PowerShell,
                program: PathBuf::from("powershell.exe"),
            };
            let pwsh = DetectedShell {
                shell_type: ShellType::PowerShell,
                program: PathBuf::from("pwsh.exe"),
            };

            assert!(windows_powershell.uses_windows_powershell_native_pipeline());
            assert!(!pwsh.uses_windows_powershell_native_pipeline());
        }

        #[cfg(not(windows))]
        {
            let shell = DetectedShell {
                shell_type: ShellType::Bash,
                program: PathBuf::from("/bin/bash"),
            };
            assert!(!shell.uses_windows_powershell_native_pipeline());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_native_rg_pipeline_requires_crlf_for_eol_anchors() {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("fixture.js"), "fixture\n").unwrap();
        let rg = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../src/resources/tools/windows-x86_64/rg.exe")
            .canonicalize()
            .unwrap();
        let rg = rg.to_string_lossy().replace('\'', "''");
        let powershell =
            PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
        let run = |command: &str| {
            std::process::Command::new(&powershell)
                .args(["-NoProfile", "-Command", command])
                .current_dir(directory.path())
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .unwrap()
        };

        let failed = run(&format!("& '{rg}' --files . | & '{rg}' 'fixture\\.js$'"));
        assert_eq!(failed.status.code(), Some(1));
        assert!(failed.stdout.is_empty());
        assert!(failed.stderr.is_empty());

        let recovered = run(&format!(
            "& '{rg}' --files . | & '{rg}' --crlf 'fixture\\.js$'"
        ));
        assert!(recovered.status.success());
        assert!(String::from_utf8_lossy(&recovered.stdout).contains("fixture.js"));
    }

    #[test]
    fn assesses_only_simple_known_shell_commands_without_approval() {
        let shell_type = platform_shell_type();
        assert_eq!(
            assess_script(shell_type, "pnpm build").risk,
            CommandRisk::BuildOrTest
        );
        assert!(!assess_script(shell_type, "pnpm build").requires_approval);
        assert_eq!(
            assess_script(shell_type, "git status --short").risk,
            CommandRisk::ReadOnly
        );
        assert!(!assess_script(shell_type, "Get-Content -Raw 'Cargo.toml'").requires_approval);
        assert_eq!(
            assess_script(shell_type, "Remove-Item -Recurse target").risk,
            CommandRisk::Destructive
        );
        assert!(assess_script(shell_type, "Remove-Item -Recurse target").requires_approval);
        assert!(assess_script(shell_type, "rg --pre=cat needle").requires_approval);
    }

    #[test]
    fn composed_or_dynamic_shell_commands_require_approval() {
        let shell_type = platform_shell_type();
        for command in [
            "Get-Content Cargo.toml | Set-Content copy.toml",
            "git status; Remove-Item target",
            "Write-Output $(Remove-Item target)",
            "echo hello > output.txt",
        ] {
            assert!(
                assess_script(shell_type, command).requires_approval,
                "{command}"
            );
        }

        #[cfg(windows)]
        assert!(assess_script(ShellType::Cmd, "echo %TEMP%").requires_approval);
    }

    #[test]
    fn interactive_launch_keeps_default_shell_alive_with_utf8_init() {
        let shell = default_user_shell();
        let (program, args) = shell.interactive_launch();
        assert!(!program.trim().is_empty());
        #[cfg(windows)]
        match shell.shell_type {
            ShellType::PowerShell => {
                assert_eq!(&args[..3], ["-NoProfile", "-NoExit", "-Command"]);
                assert!(args[3].contains("OutputEncoding"));
                assert!(args[3].contains("InputEncoding"));
            }
            ShellType::Cmd => {
                assert_eq!(args, ["/D", "/K", "chcp 65001>nul"]);
            }
        }
        #[cfg(not(windows))]
        assert!(args.is_empty());
    }

    #[test]
    fn detects_a_supported_default_shell() {
        let shell = default_user_shell();
        #[cfg(windows)]
        {
            assert!(matches!(
                shell.shell_type,
                ShellType::PowerShell | ShellType::Cmd
            ));
        }
        #[cfg(not(windows))]
        {
            assert!(matches!(
                shell.shell_type,
                ShellType::Zsh | ShellType::Bash | ShellType::Sh
            ));
        }
    }
}
