---
name: linear
description: Linear board for this repository. Use when the user asks what to work on, about tasks or issues, to plan, create, update, split, or close a task, or after a fix or push when a task may be done. Applies only when a Linear MCP server is connected; the skill finds the project bound to this repository itself.
---

# Linear

Linear calls and the binding file: [reference.md](reference.md). Issue shape: [issue-template.md](issue-template.md).

This skill applies only when a Linear MCP server is connected. Without one, do not mention Linear and do not suggest setting it up unless the user asks.

## Find the board

The repository does not name a Linear workspace, team, or project: every fork and every contributor binds their own. Resolve the binding once per session, in this order, and stop at the first hit:

1. **Local cache.** Read `.agents/local/linear.json` (gitignored, format in [reference.md](reference.md)). If the project it names still exists (`get_project`), use it.
2. **Repository link.** Get the repository URL from `git remote get-url origin` and normalize it to `https://<host>/<owner>/<repo>` (strip `.git`, turn `git@host:owner/repo` into https). Look for the Linear project whose resource links, or failing that whose description, contain that URL (procedure in [reference.md](reference.md)). Exactly one match: use it and write the cache. This is the path that works in fresh cloud checkouts, where no cache exists.
3. **Ask.** List the teams and their projects, let the user pick, write the cache. Then offer once to add the repository URL as a link on the chosen project, so step 2 finds it next time and in cloud sessions. Add it only after a yes.

Never guess between several candidates; ask. Never write the binding into a tracked file.

Once bound:

- Every `list_issues` call passes the bound `project`. Never list the whole team and filter by eye: a team usually holds other projects.
- Every new issue passes the bound `team` and `project`. An issue without the project is invisible to this workflow.
- If `get_issue` returns an issue from another project, or nothing, say so and do not touch it unless the user asks.
- Statuses and labels come from the team (`list_issue_statuses`, `list_issue_labels`), not from memory. Open work is every status of type backlog, unstarted, or started; completed and canceled types are closed.

Assign new issues to `me`. There is no delete: set a canceled status, or the user archives in the UI.

`list_issues` sorts by created or updated time, not board order. Priority is the Linear field: 1 Urgent, 2 High, 3 Medium, 4 Low, 0 None. Set it when creating.

Descriptions use real newlines, never escaped `\n`.

## Language

Write issues and comments in the language the project's existing issues use. In an empty project, use the language the user writes in. Identifiers, commands, paths, and API terms stay as they are.

## Modes

### sync -- what to do

1. `list_issues` with the bound `project`. Paginate while `hasNextPage` is true.
2. Drop closed issues.
3. Answer from started, then unstarted issues. Order by priority (1, 2, 3, 4, then 0); within a priority, started before unstarted.
4. Backlog: a count, not a dump, unless the user asks for everything.
5. For each item: identifier, title, status, priority, URL, and 2 to 4 sentences from its Status section (where it stands, what is next). Not a bare title.

A chat that is only about tasks (sync, plan, close) does not implement the work.

### add -- new task from the chat

1. Run [Check before writing](#check-before-writing).
2. Show the change plan. Create only after a yes.
3. `save_issue` with team, project, state, priority, assignee, and a description per [issue-template.md](issue-template.md).

### during work -- while implementing an issue

Every Linear call costs tokens, so write only what the next agent will actually read.

- Normal task: move it to a started status at the start and make no other updates while working. When the work is done, rewrite the description once: Status as a short report of what landed and what is left, plus Decisions, Details, and a Log line. Findings the next agent needs (a trap, a rejected option, a version that broke) go there too, not only into the PR or commit message.
- Discussion that changes the task: once it is settled and the decisions noticeably change what the issue says (scope, done-when, approach), update the description right away. Do not record each step of the discussion.
- Large task across several sessions or context compactions: intermediate results go into comments, one per milestone, so the work survives a lost context. The description is still rewritten at the end.
- Long raw material (test output, a how-to) goes into a comment.

These are updates to the issue the user already asked you to work on, so they need no separate change plan. Creating, splitting, or closing issues does.

### close -- after a fix, push, or "done"

1. Run [Evidence](#evidence--git-log) if it was not just done.
2. Propose the matching issue and ask.
3. On yes: `save_issue` with that id and a completed status. Rewrite Status to say what landed, add a Log line with the commit hash.

## Check before writing

Every create, update, split, or close goes through this first, so that one issue holds one deliverable and new facts land on the right issue.

1. **Same deliverable or a new one.** Compare with the target issue's Task section. If the fact changes what the issue delivers or what done means, it is a new issue, linked with `relatedTo`. Progress, decisions, and data about the same deliverable go into the existing issue.
2. **One deliverable per issue.** Two outcomes that can close on different days are two issues. Propose a split.
3. **Done when.** Every new issue needs one line "done when ...". If it cannot be written, it is a question to the user, not an issue.
4. **Already done or already filed.** Search the project including closed issues (`list_issues` with `project` and `query`) and check git log. Point to the existing issue instead of filing a duplicate.

Show a numbered change plan. Operations: `CREATE`, `UPDATE`, `SPLIT`, `CLOSE`, `QUESTION`, `SKIP`. Each line names the target issue, what gets written (which section), `done when` for new issues, and the checks that matter.

```text
1. CREATE   "Token endpoint: POST form grant"
   done when: containerd and helm get a token from POST /token without a GET fallback
   checks: no duplicate; GET /token stays in ABC-12
2. UPDATE   ABC-14  Log + Status
   fact: multipart parts now 16 MiB, commit abc1234
   checks: same deliverable, Task unchanged
3. QUESTION "pull-through cache for Docker Hub"
   reason: not in the project scope yet
```

The user answers by number ("1, 2 yes, skip 3"). Write only the approved lines. Updates to existing issues need approval too.

## Evidence -- git log

Use when syncing, planning closes, or answering what landed. Prefer commit messages; open code only if a message is thin. Run from the root of the current checkout.

```bash
git log --format="%h %ad %s" --date=short --since="YYYY-MM-DD"
```

Map subjects onto open issues by identifier first (`ABC-123` in the subject or branch), then by title. Mention the hash in the close note.

## Issue hygiene

- Title: a readable name, not a paragraph.
- Follow [issue-template.md](issue-template.md): the Status section first, rewritten on every update; history in Log.
- Files (specs, screenshots) go to issue attachments as originals. Other raw material goes to comments via `save_comment`. Before working on a long issue, call `list_comments`.
- No local machine paths (`C:\Users\...`, `/home/...`). Issues get picked up by cloud agents and on other machines. Use paths relative to the repository root and links to the repository host for files, commits, and PRs.
- Never copy tokens, passwords, private keys, or API keys into issues, comments, or attachments.
