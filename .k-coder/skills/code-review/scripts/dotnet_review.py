#!/usr/bin/env python3
import argparse
import json
import locale
import re
import subprocess
from pathlib import Path


def resolve_existing_path(root: Path, value: str) -> Path:
    path = Path(value)
    if not path.is_absolute():
        path = root / path
    return path.resolve(strict=True)


def first_match(base: Path, pattern: str):
    matches = sorted(p for p in base.rglob(pattern) if p.is_file())
    return matches[0] if matches else None


def resolve_build_target(root: Path, solution: str, project: str, prefer_solution: bool):
    if project:
        return {"type": "project", "path": str(resolve_existing_path(root, project))}

    if solution:
        return {"type": "solution", "path": str(resolve_existing_path(root, solution))}

    root_solutions = sorted(p for p in root.glob("*.sln") if p.is_file())
    if root_solutions:
        return {"type": "solution", "path": str(root_solutions[0].resolve())}

    if prefer_solution:
        solution_path = first_match(root, "*.sln")
        if solution_path:
            return {"type": "solution", "path": str(solution_path.resolve())}

    src_path = root / "src"
    if src_path.exists():
        project_path = first_match(src_path, "*.csproj")
        if project_path:
            return {"type": "project", "path": str(project_path.resolve())}

    solution_path = first_match(root, "*.sln")
    if solution_path:
        return {"type": "solution", "path": str(solution_path.resolve())}

    project_path = first_match(root, "*.csproj")
    if project_path:
        return {"type": "project", "path": str(project_path.resolve())}

    raise FileNotFoundError(f"No .sln or .csproj file found under '{root}'.")


def resolve_test_targets(root: Path, test_project: str):
    if test_project:
        return [str(resolve_existing_path(root, test_project))]

    targets = []
    for folder_name in ("test", "tests"):
        test_root = root / folder_name
        if test_root.exists():
            targets.extend(str(p.resolve()) for p in sorted(test_root.rglob("*.csproj")) if p.is_file())
    return targets


def run_dotnet(args, cwd: Path):
    encoding = locale.getpreferredencoding(False) or "utf-8"
    completed = subprocess.run(
        ["dotnet", *args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
        encoding=encoding,
        errors="replace",
    )
    combined = "\n".join(part for part in (completed.stdout, completed.stderr) if part).strip()
    return {
        "exitCode": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
        "combined": combined,
    }


def get_error_lines(text: str):
    if not text or not text.strip():
        return []

    patterns = (
        re.compile(r":\s+error\s+", re.IGNORECASE),
        re.compile(r"^Build FAILED\.$", re.IGNORECASE),
        re.compile(r"^\s*Failed!\s*$", re.IGNORECASE),
        re.compile(r"^\s*Error:", re.IGNORECASE),
        re.compile(r"Unhandled exception", re.IGNORECASE),
    )

    lines = []
    for line in re.split(r"\r\n|\n|\r", text):
        if any(pattern.search(line) for pattern in patterns):
            lines.append(line)
        if len(lines) >= 30:
            break
    return lines


def quote_arg(value: str):
    return '"' + value.replace('"', '\\"') + '"' if re.search(r'\s|"', value) else value


def main():
    parser = argparse.ArgumentParser(description="Build and optionally test a .NET workspace for code review.")
    parser.add_argument("--root", default=".", help="Repository root. Defaults to current directory.")
    parser.add_argument("--solution", help="Specific .sln path to build.")
    parser.add_argument("--project", help="Specific .csproj path to build.")
    parser.add_argument("--test-project", help="Specific test .csproj path to run.")
    parser.add_argument("--run-tests", action="store_true", help="Run discovered test projects after a successful build.")
    parser.add_argument("--build-solution", action="store_true", help="Prefer a recursively discovered .sln over a src project.")
    args = parser.parse_args()

    root = Path(args.root).resolve(strict=True)

    try:
        build_target = resolve_build_target(root, args.solution, args.project, args.build_solution)
        build_result = run_dotnet(["build", build_target["path"]], root)
        build_succeeded = build_result["exitCode"] == 0

        test_summaries = []
        test_executed = False
        test_succeeded = None

        if args.run_tests and build_succeeded:
            test_targets = resolve_test_targets(root, args.test_project)
            test_executed = len(test_targets) > 0
            test_succeeded = True

            for target in test_targets:
                result = run_dotnet(["test", target, "--no-build"], root)
                passed = result["exitCode"] == 0
                if not passed:
                    test_succeeded = False

                test_summaries.append(
                    {
                        "target": target,
                        "exitCode": result["exitCode"],
                        "succeeded": passed,
                        "errorLines": get_error_lines(result["combined"]),
                    }
                )

        summary = {
            "root": str(root),
            "build": {
                "targetType": build_target["type"],
                "targetPath": build_target["path"],
                "command": f"dotnet build {quote_arg(build_target['path'])}",
                "exitCode": build_result["exitCode"],
                "succeeded": build_succeeded,
                "errorLines": get_error_lines(build_result["combined"]),
            },
            "tests": {
                "requested": bool(args.run_tests),
                "executed": test_executed,
                "succeeded": test_succeeded,
                "targets": test_summaries,
            },
        }
    except Exception as exc:
        summary = {
            "root": str(root),
            "build": {
                "targetType": None,
                "targetPath": None,
                "command": None,
                "exitCode": 1,
                "succeeded": False,
                "errorLines": [str(exc)],
            },
            "tests": {
                "requested": bool(args.run_tests),
                "executed": False,
                "succeeded": None,
                "targets": [],
            },
        }

    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
