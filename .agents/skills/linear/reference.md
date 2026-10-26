# Linear reference

Tool names are those of the official Linear MCP server; another server may prefix them.

## Binding file

`.agents/local/linear.json`, gitignored, one per checkout. Written by the skill, never committed.

```json
{
  "repository": "https://github.com/<owner>/<repo>",
  "workspace": "https://linear.app/<workspace>",
  "team": {"key": "ABC", "id": "<uuid>", "name": "<team name>"},
  "project": {"name": "<project name>", "id": "<uuid>", "slug": "P-ABC-1", "url": "https://linear.app/..."},
  "boundAt": "YYYY-MM-DD"
}
```

If `repository` differs from the current `origin` URL (a fork, a renamed remote), treat the cache as missing and resolve again.

## Resolving by repository link

1. `git remote get-url origin`, normalized to `https://<host>/<owner>/<repo>`.
2. `list_projects` with `query: <repo>`, then without a query if nothing comes back. Paginate.
3. For each candidate, `get_project` and look for the URL among its resource links, then in its description. Compare case-insensitively, ignoring a trailing `/` or `.git`.
4. Exactly one project matches: that is the binding. Its team comes from the project's teams; if the project spans several teams, ask which one new issues go to.
5. None or several: fall back to asking the user.

To make a project discoverable, add the repository URL to it as a link resource (or, if the server cannot add links, a line in its description). Ask before writing.

## Calls

List open work:

```
list_issues
  project: <bound project>
  limit: 100
```

Search, including closed:

```
list_issues
  project: <bound project>
  query: <words>
```

Statuses and labels of the team:

```
list_issue_statuses
  team: <bound team>

list_issue_labels
  team: <bound team>
```

Create:

```
save_issue
  team: <bound team>
  project: <bound project>
  title: ...
  description: ...
  state: <an unstarted status of the team>
  priority: 3
  assignee: me
  labels: [...]
```

An issue created without `state` lands in the team's default status (often Backlog), so pass `state` explicitly. Without `project` the issue drops out of every sync, so never omit it.

`save_issue` creates when `id` is omitted and updates when `id` is an identifier (`ABC-123`) or uuid. `links` and `addLabels` append; `labels` replaces the whole set. There is no delete-issue tool.

## Update and close

On any update: `get_issue`, rewrite Status, add the Log line, then save the whole `description`. Do not drop existing Log lines or links.

```
save_issue
  id: ABC-123
  state: <a completed status of the team>
```

Linear normalizes markdown on save (`-` list markers may become `*`, bare URLs get `<...>`). Before a partial edit, re-read the stored text and copy anchors from it.

## Files as attachments

One file at a time; the signed upload URL lives about 60 seconds.

1. Have the file locally.
2. `prepare_attachment_upload` with `issue`, an ASCII `filename`, `contentType`, and the exact `size` in bytes.
3. `curl -X PUT --data-binary @file` with every header from `uploadRequest.headers`, verbatim.
4. `create_attachment_from_upload` with `assetUrl` and a title that says what the file is.

To embed a file in a comment instead, use `prepare_file_upload` and put the returned markdown into the comment body.

## Raw material as a comment

```
save_comment
  issueId: ABC-123
  body: |
    <what it is>, <date>

    <text, secrets removed>
```

Read comments with `list_comments` (`issueId`); `get_issue` does not include them.
