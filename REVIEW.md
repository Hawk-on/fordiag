# REVIEW.md

Review guide for fordiag. Read this before reviewing a change. It says what matters,
what to skip, and what has already been decided. The full project rules are in
`CLAUDE.md`.

fordiag is a **read-only** ELM327 diagnostic CLI for one car (Ford S-Max Mk2). A bug
that sends the wrong thing to the car matters far more than anything else.

## Review the diff, not the repository

- Start from the diff (`git diff main...HEAD`, or the PR diff). Open other files only
  to check a call site or an invariant the diff touches.
- Do not read these unless the diff changes them: `fixtures/replay/*.json(l)`
  (recorded data), `docs/BACKGROUND.md` (research notes), `Cargo.lock`.
- Every finding must cite `file:line` and name a concrete input or path to the failure.

## Blocking: check these on every change

1. **Allowlist** (`src/command.rs`)
   - Everything sent is a `Command` (`AtCommand` / `DiagRequest`). There is no variant
     for raw bytes or text, and no `Transport` method that takes `&str`/`&[u8]`.
   - No writing or session-changing service may become reachable: `04 08 10 11 14 27
     28 2E 2F 31 34–37 3D 85`, `ATCAF0`, any `ATPB` other than `81 04`, and
     `ATSH`/`ATCRA` outside 0x700–0x7FF.
   - Every transport calls `checked_wire(cmd)` before writing. Nothing ever sends an
     empty line.
   - A new allowlist variant needs tests in `command.rs`, plus updates to the
     independent definitions in `src/safety_tests.rs` (`is_allowed_form`,
     `is_allowed_at`, `all_commands`). Weakening, skipping or narrowing a safety test is
     always blocking.
2. **Serial port:** only `SerialTransport::exchange` opens or writes the port. The
   tripwire tests in `safety_tests.rs` enforce this; they must not be relaxed.
3. **No measurement without the owner:** `wait_enter` must still refuse when there is
   no keyboard input and `--yes` was not given.
4. **Privacy**
   - No VIN or registration number in code, docs, fixtures, commits or default report
     output.
   - Files named in saved results (`raw_log`) must stay plain names inside `logs/`.
5. **Honesty in output and docs**
   - Module addresses are *unverified* until a module has answered on the car.
   - DTC meanings are not included; failure-type texts are *suggestions*.
   - Nothing may claim a Ford-specific meaning for a B or U code.
6. **Parsing of car data** (`isotp.rs`, `dtc.rs`, `elm.rs`, `replay.rs`)
   - No panics on any input.
   - A responding module must never be reported as silent, or the reverse.
   - Replay divergence must stop the run (`ReplayError`), not become a result.
7. **CI stays green:** `cargo fmt --check`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test`, and `actionlint` for workflow changes.

## Not worth flagging

- Style that rustfmt and clippy (`-D warnings`) accept.
- Wording or grammar in docs, unless it is wrong about behaviour or safety.
- The emulator and `--mock` data being made up. It is labelled as such.
- Test-only code that is slow but bounded (the exhaustive 3-byte test runs in about 3 s).
- Performance: requests are rate-limited by the serial line, so micro-optimisations
  do not matter.
- Windows-specific behaviour beyond what CI builds. Windows is optional and untested on
  a car.
- Missing features from the roadmap in `CLAUDE.md`, unless the PR claims them.

## Decided, do not re-raise

- Replay is strict and in order. Run the same command with the same options as the
  recording; there is no fuzzy matching.
- `fordiag check` prints the `Debug` form of the parsed command. That is intended.
- Voltages are `f32` with 0.1 V ELM resolution. Thresholds compare with 0.001 V slack.
- Text commands accept only space and tab as separators. Control characters are
  rejected.
- OBD 03/07/0A answers are accepted with and without the count byte, told apart by
  length.
- `--port` defaults to `/dev/ttyUSB0` (Linux) and `COM3` (Windows).
- The CI emulator job installs ELM327-emulator with `GITHUB_RUN_NUMBER` unset; see the
  comment in `ci.yml`.

## Severity labels

- **Blocking:** breaks a rule above, sends something unsafe, leaks private data,
  misreports the car, or turns CI red.
- **Should fix:** a real bug with a realistic path that is not blocking.
- **Nit (optional):** only when clearly correct and cheap. Prefer not to post these.
