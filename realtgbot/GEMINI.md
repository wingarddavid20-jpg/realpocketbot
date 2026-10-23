## Terminal Working Directory & Command Execution Safety

My Windows username contains an exclamation mark (`!`) in the path
`C:\Users\Hello!\`. This character is treated as a shell metacharacter and
causes the agent's security scanner to block all `run_command` calls.

A Windows junction already exists at `C:\memescan` pointing to the real workspace.

**Rules (always enforce, no exceptions):**
- ALWAYS use `C:\memescan` as the `Cwd` for every `run_command` call.
- `C:\memescan` is a Windows junction pointing to the actual project workspace.
- NEVER use any path containing `Hello!` in any `Cwd` parameter.
- When referencing workspace files in prose or tool calls, use `C:\memescan\...`.
- ALWAYS wrap all terminal commands, file paths, and argument inputs in strict single quotes or explicit string literals during execution.
- When generating or executing scanner scripts, ensure special characters (like exclamation marks `!`) are programmatically escaped or passed via array arguments to prevent execution context leaks or history expansion blocks.
