# Rules for AI agents

These apply to every AI coding agent working in this repository (Claude Code, Codex, Cursor, Copilot, Gemini, Aider, etc.).

## Never read or write the `.git/` directory

- Do not open, read, list, search, edit, create or delete anything inside `.git/`, with any tool or shell command (`cat .git/config`, `ls .git`, `grep … .git/`, editing hooks, etc.).
- `.git/config` holds credentials (the remote URL contains an access token). Reading it leaks the token into logs and transcripts.
- Use `git` commands instead, and don't print secrets: `git status`, `git log`, `git diff`, `git branch`, `git config --get <key>` for non-secret keys. Don't run `git remote -v` or `git config --get remote.origin.url`, because those print the token.

## Other secrets

- Do not read or print `.env`, keypair files (`id.json`, `*.keypair`, `*.key`) or private keys. Refer to settings by variable name instead.
