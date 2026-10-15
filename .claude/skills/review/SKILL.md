---
name: review
description: Review a change to fordiag (a PR, a branch or the working tree) against the project's safety, privacy and correctness rules in REVIEW.md, with as little reading as possible. Use when asked to review code in this repository, or before pushing a change that touches src/, tests/ or .github/.
allowed-tools: Read, Glob, Grep, Bash(git diff *), Bash(git log *), Bash(git show *), Bash(cargo fmt --check), Bash(cargo clippy *), Bash(cargo test *), Bash(actionlint *)
---

# Reviewing a fordiag change

1. Read `REVIEW.md` in the repository root. It is the checklist: blocking rules, what not
   to flag, and decisions not to re-raise. Do not re-derive these from the code.
2. Get the diff, and nothing more yet:
   - working tree: `git diff` and `git diff --cached`
   - branch or PR: `git diff main...HEAD` (or the PR diff)
   - first only the file list: `git diff --stat main...HEAD`
3. Decide which blocking areas the diff touches, from the file list:
   - `src/command.rs`, `src/transport.rs`, `src/replay.rs`, `src/safety_tests.rs`,
     `tests/cli_safety.rs`: allowlist, serial port and replay rules. Read those hunks
     closely.
   - `src/dtc.rs`, `src/isotp.rs`, `src/elm.rs`: parsing of car data.
   - `src/report.rs`, `fixtures/`, docs: privacy and honesty rules.
   - `src/main.rs`: `wait_enter` and `--yes`, the CLI and the disclaimer.
   - `.github/workflows/`: CI stays green (`actionlint`).
   Skip the areas the diff does not touch.
4. Open surrounding code only to confirm a specific suspicion, for example a caller or
   an invariant. Use `Grep` for call sites instead of reading whole files. Do not open
   `fixtures/replay/*`, `Cargo.lock` or `docs/BACKGROUND.md` unless they are in the diff.
5. Verify before reporting. Run only what the change needs:
   - Rust changes: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
     `cargo test`
   - workflow changes: `actionlint`
   A finding needs a concrete input or path to the failure. Drop anything you cannot
   make concrete.
6. Report the findings, most severe first, using the severity labels in `REVIEW.md`
   (Blocking, Should fix, Nit). Each finding is one or two sentences with `file:line`.
   If nothing survives, say so in one line. Do not summarise the diff back to the
   author.

Reply in the owner's language (usually Norwegian Nynorsk) when talking to them. Write
in English in GitHub comments.
