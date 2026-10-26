# STYLEGUIDE.md

How code in this repository is written. Formatting is the formatter's job (`cargo fmt`). This file covers what a formatter cannot decide.

## All code

Code, comments, and string literals use only characters a standard keyboard types: `--` for an em dash, `-` for an en dash, `->` for an arrow, `...` for an ellipsis, straight quotes. Unicode is allowed only when the symbol is the payload: box-drawing output, code that searches for or validates that exact character.

Leave a blank line between logical steps of a function, not between every line.

Do not extract a helper or constant used once; inline it. Extract only when it is used in more than one place or the block is long. Name a function for what it does: a function that validates is not `normalize`.

A function does its own job and does not decide whether it should run at all; the caller decides. Do not add a second safeguard in a lower layer "just in case".

Test behavior that can break: protocol responses, merge and skip logic, config switches, edge cases of external services. Use a realistic fixture. Do not test a constant or a one-line mapper.

## Rust

### Errors

Domain modules return typed errors (`thiserror`); `anyhow` only in `main` and CLI glue. Errors that reach HTTP clients are one enum per protocol with an `IntoResponse` impl, so the wire format lives in one place.

No `unwrap` / `expect` outside tests (the crate lints warn on both). An invariant that cannot fail is still handled: map it to an internal error or restructure so the case does not exist. Tests may `#[allow(clippy::unwrap_used)]` on the test module.

### Types and async

Prefer an enum over a set of booleans, a newtype over a bare `String` when the value has rules (a digest, a repository name). Parse at the boundary into the type, then pass the type.

`async fn` in traits is used natively. Backends chosen by config are dispatched through an enum, not `Box<dyn Trait>`, until a plugin boundary needs dynamic dispatch.

### Layout

One crate. One module per concern; `mod.rs` builds the router or re-exports and stays short. Handlers are thin: parse, call the domain, map the result.

`clippy --all-targets` is clean. A lint that is wrong for a specific spot gets `#[allow(...)]` there with a comment why, not a crate-wide allow.

## Dependencies

The point is to think before `cargo add`, not to audit forever.

### Before adding a dependency

Run a subagent that looks at the new crate and at what is already installed, and answers:

- What it is for, and whether an installed crate already does it.
- How much it pulls in transitively (`cargo tree -e normal -i <crate>` after a trial add).
- Who maintains it, when the last release was, whether releases are regular.
- Whether it runs code at build time: a `build.rs` or a proc-macro.

If the job is a few dozen lines of code, write them instead of adding a library.

Install through the package manager (`cargo add`), never by editing the manifest. `Cargo.lock` is committed and every build uses `--locked`.

### Release age

`mise.toml` skips tool releases younger than 3 days (`minimum_release_age = "3d"`), because a hijacked release is usually caught and unpublished within that window. Cargo has no such setting: before `cargo add` or `cargo update`, check that the chosen version is at least 3 days old (`cargo info <crate>`, or the crate page) and pin the previous one if not.

### Scanning

`mise run security:scan` (`scripts/security-scan.sh`) runs every check and exits non-zero on findings:

- `cargo audit` -- RustSec advisories for `Cargo.lock`.
- `trivy fs` -- committed secrets and the `Containerfile`.
- `trivy image <ref>` -- only when an image ref is passed: OS packages of the base image, HIGH and CRITICAL.

Run it after adding or updating dependencies. Fix what is reachable from our code; do not chase every transitive advisory. A false positive goes into `.trivyignore` (or the matching ignore option of the audit tool, in the script) with a comment that says why it does not apply. No silent ignores.

### Overrides and patches

When an advisory sits in a transitive dependency and the parent has no fixed release, force the fixed version (`cargo update -p <crate> --precise <version>` or `[patch.crates-io]`). Verify the parent's code paths we call, not just the build. Each override or patch is listed here with the reason and the condition to remove it:

| What       | Why | Remove when |
|------------|-----|-------------|
| (none yet) |     |             |
