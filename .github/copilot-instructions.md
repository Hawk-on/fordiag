# Instructions for Copilot (code review)

fordiag is a read-only ELM327 diagnostic CLI for a Ford S-Max Mk2. The full review
guide is `REVIEW.md` in the repository root, and the project rules are in `CLAUDE.md`.
In short:

**Blocking**
- Everything sent to the cable must be a `Command` from the allowlist in
  `src/command.rs`, and must pass `checked_wire`. There is no raw-bytes path.
  Writing or session-changing services (`04 08 10 11 14 27 28 2E 2F 31 34–37 3D 85`),
  `ATCAF0`, any `ATPB` other than `81 04`, and `ATSH`/`ATCRA` outside 0x700–0x7FF must
  stay unreachable.
- Only `SerialTransport::exchange` may open or write the serial port. Safety tests
  (`src/safety_tests.rs`, `tests/cli_safety.rs`) must never be weakened or skipped.
- Measurements must not start without keyboard input unless `--yes` is given.
- No VIN or registration number anywhere. `raw_log` names in saved results must stay
  plain file names inside `logs/`.
- Module addresses are unverified, DTC meanings are not included, and failure-type
  texts are suggestions. Nothing may claim a Ford-specific meaning for a B or U code.
- Parsers of car data must not panic. A replay that diverges must stop the run.

**Do not flag**
- Style accepted by rustfmt and clippy `-D warnings`, or doc wording.
- Made-up emulator and mock data.
- Performance: the serial line is the bottleneck.
- The decisions listed under "Decided, do not re-raise" in `REVIEW.md`.

Review the diff. Cite `file:line` and a concrete input or path for each finding.
