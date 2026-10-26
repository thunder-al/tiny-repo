# Issue template

A guideline, not a form. A rename can be two lines; a task that runs for weeks uses every section. Skip a section that has nothing to say; never leave an empty heading. Write the headings in the language the project's issues use (see `SKILL.md`, Language).

Two goals:

1. The top of the description says where the task stands **now**. Someone who reads only that block knows what to do next.
2. Everything needed to do the task lives in the issue. Another agent should not have to reread old chats.

## Description, attachments, comments

- **Description** is a living document. Rewrite it when things change; do not append a block per update.
- **Attachments** hold files: specs, screenshots, exports. Upload the original, not retyped text. The description names the file and gives its essence.
- **Comments** hold raw material that is not a file: a long test report, a how-to. One comment per topic, starting with what it is and the date.
- `get_issue` lists attachments but not comments. When the description points to a comment, or the task is long, call `list_comments`.

## Sections

### `## Status (updated YYYY-MM-DD)` (required)

First thing in the description. Rewritten when the work is done or when settled decisions change the task, with the date in the heading.

A short paragraph (2 to 5 sentences): what is done, what is true now, what was agreed. Then any of these lines that apply:

- `Next:` the concrete next step and where (crate, module, endpoint).
- `Don't:` options already rejected, traps.
- `Blocked on:` what it waits for.

Absolute dates only, never "yesterday" or "last week".

### `## Task`

Why the task exists and what counts as done. One line for small tasks. For bigger ones add acceptance checks: endpoints, client commands that must pass, numbers.

### `## Decisions`

Decisions that change how the work is done, one line each: date, what, why if not obvious.

```markdown
- 2026-10-25: push is proxied through the gateway; presigned upload URLs break the OCI upload protocol
- 2026-10-25: one machine and an in-process lock; the S3 backend has no working conditional writes
```

When a decision is overturned, strike it (`~~old~~`) and add the new one with its date, or remove it and say so in Status.

### `## Details`

Only what the code does not already show: paths, config keys, env vars, commands, verification steps, gotchas.

### `## Log`

One line per episode, oldest first: date, the gist with the numbers and ids that matter, a commit hash or link.

```markdown
- 2026-10-25: client matrix against a mock registry, 9 clients x 2 auth modes -- comment in this issue
- 2026-10-26: upload sessions stream into S3 multipart -- `abc1234`
```

Every new fact goes into Log and, if it changes the picture, into the rewritten Status.

## Secrets

Never copy tokens, passwords, private keys, or API keys into the description, comments, or attachments. Write that a secret was involved and leave the value out. Check screenshots before uploading.

## Sizes

Small:

```markdown
## Status (updated 2026-10-26)
Rename `foo` to `bar` in `src/http/mod.rs`.
```

Medium: Status, Task, Log.

Long: every section, files in attachments, raw material in comments.

## Don't

- Put import notes at the top ("Imported from ...").
- Write one block per update.
- Paraphrase the original ask so much that the ask is lost.
