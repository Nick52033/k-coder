import argparse
import shutil
import subprocess
import sys
from pathlib import Path


DEFAULT_KEYWORD = "项目主数据"
SCRIPT_DIR = Path(__file__).resolve().parent
WORKSPACE = Path.cwd()
RUNNER = SCRIPT_DIR / "capture-idata-api-detail.py"
DEFAULT_OUTPUT_DIR = WORKSPACE / ".tmp-browser-session-crawler" / "output"
PROJECT_MASTER_RESPONSE = WORKSPACE / ".tmp_idata_project_master_response.json"


def main() -> int:
    parser = argparse.ArgumentParser(description="Capture iDataPlatform project master detail response.")
    parser.add_argument("keyword", nargs="?", default=DEFAULT_KEYWORD)
    parser.add_argument("--output", default=str(DEFAULT_OUTPUT_DIR))
    parser.add_argument("--profile", default=str(WORKSPACE / ".tmp-browser-session-crawler" / "profile"))
    parser.add_argument("--login-timeout", type=int, default=900)
    parser.add_argument("--detail-api-timeout", type=int, default=120)
    parser.add_argument("--action-timeout", type=int, default=30)
    parser.add_argument("--keep-open", action="store_true")
    parser.add_argument("--keep-open-on-error", action="store_true")
    args = parser.parse_args()

    output_dir = Path(args.output)
    command = [
        sys.executable,
        str(RUNNER),
        args.keyword,
        "--output",
        str(output_dir),
        "--profile",
        args.profile,
        "--login-timeout",
        str(args.login_timeout),
        "--detail-api-timeout",
        str(args.detail_api_timeout),
        "--action-timeout",
        str(args.action_timeout),
    ]
    if args.keep_open:
        command.append("--keep-open")
    if args.keep_open_on_error:
        command.append("--keep-open-on-error")

    completed = subprocess.run(command, cwd=str(WORKSPACE))
    if completed.returncode != 0:
        return completed.returncode

    captured_json = output_dir / "getApiByDeatail.json"
    if not captured_json.exists():
        print(f"Captured JSON was not found: {captured_json}", file=sys.stderr)
        return 1

    shutil.copyfile(captured_json, PROJECT_MASTER_RESPONSE)
    print(f"SAVED_PROJECT_MASTER_RESPONSE {PROJECT_MASTER_RESPONSE}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
