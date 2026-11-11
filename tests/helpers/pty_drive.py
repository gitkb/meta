"""Run a command on a terminal of its own, as a user at a shell does, and act on it.

usage: pty_drive.py ROWS COLS ACTIONS COMMAND...

ACTIONS is a ;-separated list, done in order while the command's output is read:
  wait:SECONDS        let it run
  type:TEXT           type TEXT (\\r is Enter, ^C ^Z ^D the control keys)
  resize:ROWSxCOLS    resize the window
  until:PATH          wait (up to 10 s) until PATH exists
  kill9:PATH          SIGKILL the pid written in PATH
  exit:SECONDS        wait up to SECONDS for the command to end

Prints what the command wrote, then `[exit N]` (or `[timeout]`, after which it
is killed), then the terminal's modes once it ended: `[tty canonical echo]`.

As at a shell, a session leader starts the command in a foreground group of
its own and outlives it, to read the terminal's modes: a terminal is gone
with its session's leader (on macOS at once).
"""
import fcntl
import os
import select
import signal
import struct
import sys
import termios
import time

rows, cols, actions, argv = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4:]
master, slave = os.openpty()
fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
report_r, report_w = os.pipe()
pid = os.fork()
if pid == 0:
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2):
        os.dup2(slave, fd)
    os.close(master)
    os.close(report_r)
    signal.signal(signal.SIGTTOU, signal.SIG_IGN)
    command = os.fork()
    if command == 0:
        os.close(report_w)
        os.setpgid(0, 0)
        os.tcsetpgrp(0, os.getpgrp())
        for sig in (signal.SIGINT, signal.SIGTTOU, signal.SIGTSTP):
            signal.signal(sig, signal.SIG_DFL)
        os.execvp(argv[0], argv)
    _, ended = os.waitpid(command, 0)
    lflag = termios.tcgetattr(0)[3]
    os.write(report_w, f"{os.waitstatus_to_exitcode(ended)} {lflag}".encode())
    os._exit(0)
os.close(report_w)

out = b""
status = None


def pump(seconds):
    global out, status
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([master], [], [], 0.05)
        if r:
            try:
                out += os.read(master, 65536)
            except OSError:
                pass
        if status is None:
            done, st = os.waitpid(pid, os.WNOHANG)
            if done:
                status = st
        if status is not None and not r:
            return True
    return status is not None


keys = {"^C": "\x03", "^Z": "\x1a", "^D": "\x04", "\\r": "\r"}
for action in actions.split(";"):
    kind, _, arg = action.partition(":")
    if kind == "wait":
        pump(float(arg))
    elif kind == "type":
        for name, key in keys.items():
            arg = arg.replace(name, key)
        os.write(master, arg.encode())
    elif kind == "resize":
        r, c = arg.split("x")
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", int(r), int(c), 0, 0))
    elif kind == "until":
        end = time.time() + 10
        while not os.path.exists(arg) and time.time() < end:
            pump(0.1)
    elif kind == "kill9":
        with open(arg) as f:
            os.kill(int(f.read().split()[0]), signal.SIGKILL)
    elif kind == "exit":
        pump(float(arg))
pump(0.3)
sys.stdout.write(out.decode(errors="replace"))
if status is None:
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    print("\n[timeout]")
    sys.exit(0)
code, lflag = os.read(report_r, 100).decode().split()
lflag = int(lflag)
print(f"\n[exit {code}]")
print(
    "[tty"
    + (" canonical" if lflag & termios.ICANON else " raw")
    + (" echo" if lflag & termios.ECHO else " noecho")
    + "]"
)
