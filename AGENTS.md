# AGENTS.md

## Commits

After finishing a piece of work, commit features, bug fixes and other changes
separately, one commit each. Never bundle several fixes into one commit, and
do not mix unrelated test or cleanup changes into a fix commit.

- Every commit must build. Plain `cargo test` should pass for each commit;
  a test that only shows the bug goes in with its fix, not before it.
- Use "Fix: ..." for bug fixes and "Test: ..." for test-only commits.
- Run `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings`
  before committing.
- Leave `.antigravitycli/` untracked.
