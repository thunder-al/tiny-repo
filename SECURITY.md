# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately through GitHub: the repository's **Security** tab -> **Report a vulnerability**. Include what is affected, how to reproduce it, and the impact you see.

You can expect an acknowledgement within a few days. Fixes are released as soon as they are ready, and the advisory is published after users have had a chance to update.

## Supported versions

The project is pre-1.0: only the latest release and `main` get security fixes.

## For contributors

How dependencies are vetted, scanned, and pinned is described in the Dependencies section of [`STYLEGUIDE.md`](./STYLEGUIDE.md). Secrets never go into code, commits, issues, PR text, or logs; local configuration lives in `.env` and `tiny-repo.toml`, both gitignored.
