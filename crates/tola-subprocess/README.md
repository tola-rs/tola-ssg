# tola-subprocess

Runs external commands with process-tree cleanup, cancellation, and bounded stdout/stderr capture.

- `Command::run` spawns with null stdin, isolates the child in a process group (Unix) or job object (Windows), caps captured head/tail bytes, and kills the tree on exit or cancellation.
- `Command::run_inherited` inherits stdio with the same cleanup and cancellation (lending the foreground terminal on Unix).
- Commands are not sandboxed; both methods block the calling thread.
