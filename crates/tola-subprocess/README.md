# tola-subprocess

Runs trusted external commands with bounded output capture, process-tree containment, and
cancellation. A `Command` runs one declared program: it owns the child's process group (Unix) or
job object (Windows), drains both pipes keeping a bounded head and tail, relays what it reads to
an `Observer`, and kills the whole tree when the command ends or you cancel.

- Containment is set up at spawn, before the command can fork; stdin is null.
- Each stream keeps a fixed head and tail and reports how many bytes it dropped.
- Reader threads are joined before `Command::run` returns or unwinds.
- A reader failure terminates the run even if the command is still running.
- Cancellation is an outcome, not an error: `Exit` reports either the observed status or
  `Stop::Cancelled`, and a cancelled run still reports what it captured and any observed exit
  status.

`Command::run_inherited` explicitly inherits stdin, stdout, and stderr and returns `Stop`,
without captures or reader threads. It keeps the same containment and cancellation. On Unix,
a foreground caller lends its controlling terminal to the child's group before the program
starts; the foreground group and terminal settings are restored on exit, failure, or unwinding.
The caller must not use that terminal during the run. Concurrent terminal owners and background
callers are rejected; terminal job suspension is not managed by this API.

There is no sandbox: a command's side effects are its own. Both run methods block the calling
thread. Descendants that leave the process group or job are outside its containment.
