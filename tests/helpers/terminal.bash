# Run commands with or without a terminal, for editors that need one.

# Run a command on a terminal of its own (a pseudo-terminal it is in the
# foreground of), as a user at a shell is; the editor needs one
with_terminal() {
    python3 -c 'import os, pty, sys; sys.exit(os.waitstatus_to_exitcode(pty.spawn(sys.argv[1:])))' "$@"
}

# Run a command on a terminal, but in its background (as under `timeout`)
in_background() {
    python3 -c 'import os, pty, sys; sys.exit(os.waitstatus_to_exitcode(pty.spawn([sys.executable, "-c", "import os, sys\npid = os.fork()\nif pid == 0:\n    os.setpgid(0, 0)\n    os.execvp(sys.argv[1], sys.argv[1:])\nsys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))", *sys.argv[1:]])))' "$@"
}

# Run a command with no terminal at all, as an agent or a CI job is
without_terminal() {
    python3 -c 'import os, sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])' "$@" </dev/null
}
