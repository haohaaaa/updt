# Internal terminal execution API

`src/terminal/execution.rs` exposes the internal async entry point used by the
terminal workflow:

```rust
let result = terminal::execution::execute(
    "npm",
    &["outdated", "--json", "--global", "--prefer-online"],
    terminal::execution::Mode::Capture,
).await?;

let packages: serde_json::Value = serde_json::from_slice(&result.stdout)?;
// npm may return 1 when outdated packages are present. Interpret exit codes
// according to the command, rather than treating every nonzero code as failure.
```

For an interactive command:

```rust
let result = terminal::execution::execute(
    "rustup",
    &["update", "--no-self-update"],
    terminal::execution::Mode::Interactive,
).await?;

// result.code: actual child exit code
// result.terminal_output: merged PTY bytes, including ANSI and input echo
```

## Result contract

| Mode | Execution | Returned output | Input |
| --- | --- | --- | --- |
| `Capture` | Process with stdout/stderr pipes | Separate byte-exact `stdout` and `stderr`; `terminal_output` is empty | stdin is closed |
| `Interactive` | Process in a native PTY | `terminal_output` contains merged terminal bytes; `stdout` and `stderr` are empty | Keyboard/paste forwarded to the selected live session |

PTY output is terminal data. It may contain colors, carriage returns, cursor
movement, and echoed input. Use capture mode for machine-readable JSON. The UI
command label and exit-status decoration are never added to returned output.

Each request executes once. Output is sent both to its session's terminal parser
and to the result collector. The UI continues servicing all sessions while the
workflow awaits a result. A failed spawn or transport returns `io::Error`;
ordinary nonzero exits return `CommandResult` with their actual code.

## Process ownership

The outer UI owns command processes, input routing, terminal sizes and history.
The existing workflow remains in its own PTY for confirmations and summaries.
A private, authenticated loopback connection carries requests and results
between these two updt processes. This is an internal transport, with a fresh
endpoint and token per UI launch; it is not a public HTTP API.

Program and arguments travel as separate values, using the existing command
resolution rules. The UI shows the command automatically without synthesizing
keystrokes into a shell. Each command is an independent process with the
caller's working directory. There is no persistent shell state between requests.

Existing business entry points are adapted automatically when the terminal
owner is available:

- `run_output` / `run_capture`: capture mode.
- `run_inherit` / `run_capture_streaming`: interactive mode.
- The legacy interface retains its existing local execution path.

Interactive completion drains remaining PTY output before returning the result.
Closing the UI releases PTYs and cancels active captured commands. Historical
screens use bounded scrollback; completed sessions are evicted when the session
list reaches 32. API results are retained until delivered; the internal result
frame is limited to 128 MiB of serialized data and returns an error if exceeded.

## Safe demonstration

```powershell
cargo run -- terminal --demo
```

The demo validates separate JSON/stdout and stderr capture, then accepts input
in an interactive command. Use F5/F6 to inspect either command afterward. No
package manager or update operation is run by this demo.
