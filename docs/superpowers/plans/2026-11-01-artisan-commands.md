# Artisan implementation plan

Design: `../specs/2026-11-01-artisan-commands-design.md`.

1. Add native CLI parsing, safe project filesystem helpers, secure and idempotent
   environment initialization, explicit key rotation, and Cargo package commands.
2. Add validated scaffold templates and module registration, including dual-driver
   repositories and paired migrations. Test real filesystem preservation/refusal.
3. Add Unix process supervision with Cargo artifact discovery and bounded cleanup.
   Test real subprocesses and failure paths.
4. Expose commands through safe Make wrappers; retain existing targets; document
   quickstart, command reference, manual wiring, and support boundaries.
5. Compile generated artifacts with default and single database features. Run
   local formatting, lint, unit/integration checks, and release build. Review and
   address concrete findings before publishing the exact verified tree to main.

The filesystem/setup, generators, supervisor, and Make/documentation work can
proceed independently with explicit file ownership. Cargo build/test runs are
coordinated to avoid redundant work and shared target lock contention.
