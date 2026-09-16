#!/usr/bin/env bash
# Runs the cargo lib-test binary from a *stable snapshot*.
#
# A second, concurrent cargo session in this repo repeatedly relinks
# `target/debug/deps/k_coder_lib-<hash>.exe` in place. Running the file directly then loads a
# half-written image and the Windows loader fails with STATUS_ENTRYPOINT_NOT_FOUND (Git Bash exit
# 127). Copying is not enough either: the copy can catch the same half-written state.
#
# So: snapshot, verify the snapshot is byte-stable, verify it actually starts (`--list`), and only
# then run the requested filter. Retries a bounded number of times.
set -u

EXE="${KC_TEST_EXE:-src-tauri/target/debug/deps/k_coder_lib-34dce356d94490c0.exe}"
SNAPSHOT="${KC_TEST_SNAPSHOT:-/tmp/kc-lib-test.exe}"
ATTEMPTS="${KC_TEST_ATTEMPTS:-40}"

size_of() { stat -c %s "$1" 2>/dev/null || echo 0; }

for attempt in $(seq 1 "$ATTEMPTS"); do
  cp "$EXE" "$SNAPSHOT" 2>/dev/null || { sleep 1; continue; }
  first=$(size_of "$SNAPSHOT")
  sleep 0.4
  second=$(size_of "$SNAPSHOT")
  if [ "$first" != "$second" ] || [ "$first" -lt 1000000 ]; then
    sleep 1
    continue
  fi
  if "$SNAPSHOT" --list >/dev/null 2>&1; then
    echo "[stable snapshot after $attempt attempt(s), $first bytes]" >&2
    "$SNAPSHOT" "$@"
    exit $?
  fi
  sleep 1
done

echo "could not obtain a stable snapshot of $EXE after $ATTEMPTS attempts" >&2
exit 1
