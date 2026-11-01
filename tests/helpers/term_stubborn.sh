#!/usr/bin/env bash
# term_stubborn.sh META_BIN TEST_DIR
# Runs `meta git worktree exec stubborn` in the background with a command that
# ignores TERM, sends meta TERM as another process would, and appends to
# TEST_DIR/result "ended" once meta is gone and "stubborn ended" once the
# command is.
meta=$1 dir=$2
cd "$dir" || exit 1
"$meta" git worktree exec stubborn --repo backend -- sh -c \
    'trap "" TERM; echo $$ > "$0/stubborn"; while [ -d "$0" ]; do sleep 0.05; done' "$dir" &
pid=$!
for _ in $(seq 200); do [ -s "$dir/stubborn" ] && break; sleep 0.1; done
kill -TERM "$pid"
# meta kills what is left 2 s after the signal and ends 2 s after that
for _ in $(seq 100); do
    kill -0 "$pid" 2>/dev/null || { echo ended >> "$dir/result"; break; }
    sleep 0.1
done
wait "$pid"
stubborn=$(cat "$dir/stubborn")
for _ in $(seq 50); do
    state=$(ps -o stat= -p "$stubborn")
    if [ -z "$state" ] || [ "${state#Z}" != "$state" ]; then
        echo "stubborn ended" >> "$dir/result"
        break
    fi
    sleep 0.1
done
