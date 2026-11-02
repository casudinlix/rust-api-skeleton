# Changelog

Semua perubahan penting pada skeleton dicatat di sini.

Format mengikuti Keep a Changelog dan versi mengikuti Semantic Versioning.

## [Unreleased]

### Added
- Runtime `artisan doctor` / `make doctor` preflight untuk toolchain,
  configuration, database + migration readiness, optional Redis cache/queue,
  filesystem write probe, storage link, file logging, Swagger, observability, dan
  status pre-commit hook.
- Versioned Git pre-commit hook di `.githooks/pre-commit`.
- `artisan hooks:install` / `make hooks-install` untuk mengaktifkan
  `core.hooksPath=.githooks` secara eksplisit.
- `make precommit` sebagai local quality gate sebelum commit.
- Release checklist untuk candidate `v0.1.0`.

## Release policy

- `v0.1.x`: bugfix dan hardening kompatibel.
- `v0.2.x`: capability baru yang tetap backward-compatible.
- `v1.0.0`: public skeleton contract dianggap stabil.

Saat sebuah release ditag, pindahkan perubahan relevan dari `[Unreleased]` ke
section versi dengan tanggal release.
