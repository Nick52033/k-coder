import os
import subprocess
import sys
from pathlib import Path


PROFILE_DIR = (Path.cwd() / ".tmp-browser-session-crawler" / "profile").resolve()
RUNNER_NAMES = (
    "capture-idata-api-detail.py",
    "capture-idata-project-master.py",
    ".tmp-browser-session-crawler-run.py",
    ".tmp_capture_idata_project_master.py",
)


def run_powershell(script: str) -> int:
    completed = subprocess.run(
        [
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        text=True,
    )
    return completed.returncode


def main() -> int:
    if os.name != "nt":
        print("This helper currently targets Windows process cleanup.")
        return 0

    profile = str(PROFILE_DIR).replace("'", "''")
    profile_escaped = profile.replace("\\", "\\\\")
    escaped_runner_names = [name.replace("'", "''") for name in RUNNER_NAMES]
    runner_checks = " -or ".join(
        f"($_.CommandLine -like '*{name}*')" for name in escaped_runner_names
    )
    script = f"""
$profile = '{profile}'
$currentPid = $PID
Get-CimInstance Win32_Process |
  Where-Object {{
    $_.ProcessId -ne $currentPid -and (
      {runner_checks} -or
      ($_.CommandLine -like "*--user-data-dir=$profile*") -or
      ($_.CommandLine -like "*{profile_escaped}*")
    )
  }} |
  ForEach-Object {{
    try {{
      Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop
      Write-Output "Stopped $($_.ProcessId) $($_.Name)"
    }} catch {{
      Write-Output "Failed $($_.ProcessId) $($_.Name): $($_.Exception.Message)"
    }}
  }}
"""
    return run_powershell(script)


if __name__ == "__main__":
    sys.exit(main())
