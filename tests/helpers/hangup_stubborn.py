"""hangup_stubborn.py META_BIN TEST_DIR

Runs `meta git worktree exec stubborn` as the foreground job of a terminal of
its own (a pty), with a command that ignores SIGHUP and SIGTERM, then closes
the terminal: the kernel sends SIGHUP to the foreground job, from no process.
Appends to TEST_DIR/result "ended" once meta is gone and "stubborn ended" once
the command is.
"""
import os
import pty
import select
import signal
import subprocess
import sys
import time

meta, test_dir = sys.argv[1], sys.argv[2]
command = (
    'trap "" HUP TERM; echo $$ > "$0/stubborn"; '
    'while [ -d "$0" ]; do sleep 0.05; done'
)


def result(line):
    with open(os.path.join(test_dir, "result"), "a") as out:
        out.write(line + "\n")


def gone(pid):
    state = subprocess.run(
        ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True
    ).stdout.strip()
    return state == "" or state.startswith("Z")


pid, master = pty.fork()
if pid == 0:
    os.chdir(test_dir)
    # a hangup ignored here (bats under nohup) would stay ignored in meta
    signal.signal(signal.SIGHUP, signal.SIG_DFL)
    # meta is this terminal's foreground job, and its session's leader
    os.execv(meta, [meta, "git", "worktree", "exec", "stubborn", "--repo",
                    "backend", "--", "sh", "-c", command, test_dir])

stubborn_file = os.path.join(test_dir, "stubborn")
deadline = time.monotonic() + 20
while not (os.path.exists(stubborn_file) and open(stubborn_file).read().strip()):
    if time.monotonic() > deadline:
        sys.exit("the command didn't start")
    # keep the terminal drained, without blocking on it
    if select.select([master], [], [], 0.05)[0]:
        try:
            os.read(master, 4096)
        except OSError:
            pass
stubborn = int(open(stubborn_file).read().strip())

os.close(master)  # the terminal goes: SIGHUP to its foreground job
# meta kills what is left 2 s after the hangup and ends 2 s after that
deadline = time.monotonic() + 10
while time.monotonic() < deadline:
    done, _ = os.waitpid(pid, os.WNOHANG)
    if done:
        result("ended")
        break
    time.sleep(0.1)
else:
    os.kill(pid, 9)  # fail rather than hang
    os.waitpid(pid, 0)

deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    if gone(stubborn):
        result("stubborn ended")
        break
    time.sleep(0.1)
