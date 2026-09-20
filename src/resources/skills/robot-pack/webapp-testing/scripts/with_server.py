#!/usr/bin/env python3
"""Start local servers, wait until they answer, run one command, then stop them.

This harness is maintained inside k-Coder for the `webapp-testing` Skill. It is a
local implementation that uses only the Python standard library: it installs
nothing, downloads nothing, and never reaches a remote host. Readiness is probed
on loopback addresses only.

The host never executes this file. It is a read-only Skill resource that the model
may only run through `run_command`, where policy assessment, sandbox rules,
timeouts, and user approval still apply.

Usage:
    python with_server.py --server "npm run dev" --port 5173 -- python check.py

    python with_server.py \
        --server "cd backend && python -m uvicorn app:main --port 3000" --port 3000 \
        --server "cd frontend && npm run dev" --port 5173 \
        -- python check.py

Exit codes:
    0    the wrapped command succeeded
    2    usage error
    3    a server did not become ready before its timeout
    n    the exit code of the wrapped command
"""

import argparse
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

DEFAULT_ADDRESSES = ("127.0.0.1", "::1")
DEFAULT_LOG_LINES = 40
PROBE_INTERVAL_SECONDS = 0.4


def probe_tcp(address, port, timeout=0.6):
    family = socket.AF_INET6 if ":" in address else socket.AF_INET
    try:
        with socket.socket(family, socket.SOCK_STREAM) as probe:
            probe.settimeout(timeout)
            return probe.connect_ex((address, port)) == 0
    except OSError:
        return False


def probe_http(address, port, path, timeout=2.0):
    host = "[%s]" % address if ":" in address else address
    url = "http://%s:%d%s" % (host, port, path)
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return response.status < 500
    except urllib.error.HTTPError as error:
        return error.code < 500
    except Exception:  # noqa: BLE001 - any transport failure means "not ready"
        return False


def start_server(command, cwd, log_path):
    handle = open(log_path, "w", encoding="utf-8", errors="replace")
    options = {
        "shell": True,
        "cwd": cwd or None,
        "stdin": subprocess.DEVNULL,
        "stdout": handle,
        "stderr": subprocess.STDOUT,
    }
    if os.name == "nt":
        options["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        options["start_new_session"] = True
    process = subprocess.Popen(command, **options)
    return process, handle


def wait_until_ready(server, timeout, ready_http):
    deadline = time.monotonic() + timeout
    targets = "%s on port %d" % (", ".join(server["addresses"]), server["port"])
    while True:
        code = server["process"].poll()
        if code is not None:
            return False, "server exited with code %s before becoming ready" % code
        for address in server["addresses"]:
            if ready_http:
                ready = probe_http(address, server["port"], ready_http)
            else:
                ready = probe_tcp(address, server["port"])
            if ready:
                return True, ""
        if time.monotonic() >= deadline:
            return False, "no response from %s within %ss" % (targets, timeout)
        time.sleep(PROBE_INTERVAL_SECONDS)


def stop_server(server):
    process = server["process"]
    handle = server["handle"]
    if process.poll() is None:
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
        else:
            terminate_group(process, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            if os.name != "nt":
                terminate_group(process, signal.SIGKILL)
            else:
                process.kill()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
    handle.close()


def terminate_group(process, number):
    try:
        os.killpg(os.getpgid(process.pid), number)
    except (ProcessLookupError, PermissionError, OSError):
        pass


def tail(log_path, lines):
    try:
        with open(log_path, "r", encoding="utf-8", errors="replace") as handle:
            content = handle.read().splitlines()
    except OSError:
        return []
    return content[-lines:] if lines > 0 else []


def report_failure(servers, message, lines):
    print("error: %s" % message, file=sys.stderr)
    for index, server in enumerate(servers):
        print(
            "\n--- server %d/%d: %s (port %d) ---"
            % (index + 1, len(servers), server["command"], server["port"]),
            file=sys.stderr,
        )
        for line in tail(server["log_path"], lines):
            print(line, file=sys.stderr)


def parse_arguments(argv):
    parser = argparse.ArgumentParser(
        description="Run one command with ready local servers, then clean up.",
    )
    parser.add_argument(
        "--server",
        action="append",
        dest="servers",
        required=True,
        help="Server command to start (repeatable, paired with --port)",
    )
    parser.add_argument(
        "--port",
        action="append",
        dest="ports",
        type=int,
        required=True,
        help="Port the matching --server listens on (repeatable)",
    )
    parser.add_argument(
        "--cwd",
        default="",
        help="Working directory for the server commands (default: current directory)",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=60,
        help="Seconds to wait for each server to become ready (default: 60)",
    )
    parser.add_argument(
        "--address",
        action="append",
        dest="addresses",
        help="Loopback address to probe (repeatable, default: 127.0.0.1 and ::1)",
    )
    parser.add_argument(
        "--ready-http",
        default="",
        help="Probe this HTTP path instead of a raw TCP connection",
    )
    parser.add_argument(
        "--server-log-lines",
        type=int,
        default=DEFAULT_LOG_LINES,
        help="Server log lines to print when a server fails (default: 40)",
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Command to run once every server is ready; separate it with --",
    )
    args = parser.parse_args(argv)

    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("no command given; pass it after --")
    if len(args.servers) != len(args.ports):
        parser.error("--server and --port must be repeated the same number of times")
    if args.timeout <= 0:
        parser.error("--timeout must be a positive number of seconds")
    if any(port <= 0 or port > 65535 for port in args.ports):
        parser.error("--port must be within 1..65535")

    addresses = args.addresses or list(DEFAULT_ADDRESSES)
    return args, command, addresses


def main(argv):
    args, command, addresses = parse_arguments(argv)
    log_directory = tempfile.mkdtemp(prefix="kcoder-with-server-")
    servers = []
    exit_code = 0
    try:
        for index, (server_command, port) in enumerate(zip(args.servers, args.ports)):
            log_path = os.path.join(log_directory, "server-%d.log" % (index + 1))
            print("starting server %d/%d: %s" % (index + 1, len(args.servers), server_command))
            process, handle = start_server(server_command, args.cwd, log_path)
            server = {
                "command": server_command,
                "port": port,
                "addresses": addresses,
                "process": process,
                "handle": handle,
                "log_path": log_path,
            }
            servers.append(server)
            started = time.monotonic()
            ready, reason = wait_until_ready(server, args.timeout, args.ready_http)
            if not ready:
                exit_code = 3
                report_failure(servers, "server %d did not become ready: %s" % (index + 1, reason), args.server_log_lines)
                return exit_code
            print(
                "server %d/%d ready on port %d after %.1fs"
                % (index + 1, len(args.servers), port, time.monotonic() - started)
            )

        print("\nrunning: %s\n" % " ".join(command))
        sys.stdout.flush()
        result = subprocess.run(command, cwd=args.cwd or None)
        exit_code = result.returncode
        return exit_code
    finally:
        for server in reversed(servers):
            stop_server(server)
        print("\nstopped %d server(s)" % len(servers))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
