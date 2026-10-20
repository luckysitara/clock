#!/usr/bin/env bash
# PreToolUse hook (Bash): refuse shell commands that touch the .git/ directory directly
# (e.g. `cat .git/config`, which exposes the token in the remote URL). Normal `git ...`
# commands are allowed; `.gitignore`, `.github/` and `repo.git` URLs do not match.
cmd=$(jq -r '.tool_input.command // empty')

if printf '%s' "$cmd" | grep -Eq '(^|[[:space:]"'"'"'=:;|&(<>/])\.git([/[:space:]"'"'"';|&)<>]|$)'; then
  jq -n '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: "Direct access to .git/ is not allowed in this repo (it holds credentials). Use git commands instead (`git status`, `git log`, `git diff`); never print the remote URL, it contains a token. See AGENTS.md."
    }
  }'
fi
exit 0
