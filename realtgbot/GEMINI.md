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

## Invariant: Symmetrical Admin and User Trading Implementation

Whenever implementing, modifying, or fixing trading functionality in `realtgbot`:
1. **Zero Discrepancy**: All calculations (e.g. 100% margin buffer reserves, fee deduction, tick/price bounds) must use the identical logic for both default/admin and user flows.
2. **Identical Retries**: Any error-handling or retry loop added to `place_market_long`, `place_market_short`, `place_limit_long`, or `place_limit_short` MUST be simultaneously applied to `place_user_market_long`, `place_user_market_short`, `place_user_limit_long`, and `place_user_limit_short`.
3. **No Unilateral Fixes**: Never declare a trading bug resolved until both the default/admin path and the per-user path have been updated, compiled, and verified with identical logic.

