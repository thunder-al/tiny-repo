@AGENTS.md

Project skills live in `.agents/skills/*/SKILL.md` (`.claude/skills` is a symlink to it). If the symlink did not resolve, list `.agents/skills/` and read the matching `SKILL.md` before the task it covers.

Per-user material that must not be committed (the Linear binding cache, private skills) lives in `.agents/local/`, which is gitignored. If `.agents/local/private/` exists, list it: skills there apply to this user the same way as the ones in `.agents/skills/`.
