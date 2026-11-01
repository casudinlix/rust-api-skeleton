# Laravel-style development commands

## Goal and authorization

Add familiar `make install`, `make serve`, generators, and dependency commands to
the existing Rust API skeleton. Implement a native Rust `artisan` binary and keep
Make as a thin convenience layer. The user authorized committing this work to
`main` and explicitly made GitHub CI informational; local verification is the gate.

## Command contract

- `install` initializes an absent `.env` from the example, generates a missing or
  placeholder JWT secret, and fetches/builds locked dependencies. Existing valid
  secrets and unrelated configuration survive repeated runs. It never migrates a
  database. `key:generate --force` explicitly rotates an existing key.
- `serve` builds once, starts API and worker, and optionally starts the scheduler.
  On Unix it supervises process groups, handles INT/TERM, stops siblings after an
  unexpected exit, and bounds shutdown before killing/reaping remaining children.
  Cargo artifact messages supply executable paths, including custom target dirs.
- `make:controller`, `make:model`, `make:request`, `make:service`,
  `make:repository`, `make:job`, `make:middleware`, and `make:migration` produce
  compilable scaffolds. Rust modules are registered, but routes and job dispatch
  require explicit application wiring. Repositories and migrations support both
  PostgreSQL and MySQL.
- `require`, `remove`, and `update` use Cargo argv directly. Package installation
  remains explicit, separate from repeatable project initialization.

## Boundaries

Development commands do not load application `Settings` or require live database,
Redis, or SMTP connections. The server processes retain their existing settings,
authorization, SQL queue, and cache behavior. The production image does not need
the developer CLI. Serve is not a file watcher.

Names are validated before paths are derived. Generators refuse overwrite and
existing module collisions, preserve surrounding module code, and preflight all
files. Filesystem changes use an advisory project lock and atomic per-file writes;
this is not a crash-atomic multi-file filesystem transaction. Symlink destinations
and traversal are rejected. `.env` writes use restrictive Unix permissions and
never print generated secrets. Migration versions are paired and monotonic.

Make parameters travel as literal environment data, never as interpolated shell
commands. Native commands use argv arrays without a shell. Make's Docker driver
default must not override the application's `.env` database choice.

## Verification

Exercise idempotent install and explicit key rotation; overwrite, traversal,
symlink and collision refusal; literal Make argument handling; compiled generated
modules with both database features individually; and real supervised processes.
Run `make check` and a locked release build. Use disposable service instances for
an API/worker smoke test where available. Review the final diff and publish the
verified tree without waiting for hosted CI quota availability.
