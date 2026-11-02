# Release checklist

Checklist ini adalah gate sebelum menandai skeleton sebagai baseline release.
Untuk candidate pertama gunakan tag `v0.1.0`.

## 1. Working tree dan toolchain

```bash
git status --short
rustc --version
cargo --version
make doctor
```

Harapan:

- working tree bersih;
- Rust mengikuti `rust-toolchain.toml` (1.99.1);
- `make doctor` berakhir dengan `READY`;
- warning doctor yang memang opsional ditinjau, bukan diabaikan otomatis.

## 2. Local quality gate

```bash
make precommit
```

Gate ini menjalankan:

```text
make check
  -> cargo fmt --check
  -> clippy -D warnings
  -> unit/integration-nonignored tests
  -> doc tests
  -> postgres-only compile
  -> mysql-only compile

make test-scaffolds
  -> generate scaffold reference
  -> compile/verify generated output
```

Tidak boleh ada warning compiler/linter.

## 3. Database matrix

Gunakan database disposable, bukan production.

```bash
TEST_DATABASE_URL=... make test-postgres
TEST_DATABASE_URL=... make test-mysql
```

Verifikasi migration fresh, latest rollback → forward migrate lagi, seeder default,
serta upgrade path yang relevan. `make test-postgres` dan `make test-mysql`
menjalankan regression `database_maintenance` pada database disposable.

## 4. Redis matrix

Gunakan Redis disposable terpisah dari instance production.

```bash
TEST_REDIS_URL=... make test-redis
TEST_REDIS_URL=... make test-redis-transport
```

Untuk queue Redis + SQL outbox:

```bash
TEST_DATABASE_URL=... TEST_REDIS_URL=... make test-queue-postgres
TEST_DATABASE_URL=... TEST_REDIS_URL=... make test-queue-mysql
```

## 5. Docker/runtime

```bash
make docker-build
docker compose config
```

Smoke test minimal:

1. migration process sukses;
2. API `/health` dan `/ready` sehat;
3. worker ready;
4. scheduler ready;
5. relay ready bila dipakai;
6. SIGTERM menghasilkan graceful shutdown;
7. writable `/app/storage` bekerja pada read-only root filesystem.

## 6. Filesystem/logging

```bash
make storage-link
make doctor
```

Uji bila file logging diaktifkan:

- per-process filename;
- rotation;
- retention;
- shutdown flush;
- multi-replica tetap memakai stdout + centralized collector.

## 7. Swagger

Dengan `SWAGGER_ENABLED=false`:

```text
/docs                     -> 404
/api-docs/openapi.json    -> 404
```

Dengan `SWAGGER_ENABLED=true`:

- UI tersedia;
- OpenAPI JSON tersedia;
- bearer scheme sesuai runtime;
- documented method/path tidak drift dari Axum route;
- examples tidak membawa secret/data nyata.

## 8. Security review cepat

Pastikan tidak ada:

- `.env`, credential, private key, token, DSN atau production data di Git;
- `SELECT *`, generic PK baru, OFFSET pagination atau UUID non-v7 pada owned PK;
- log raw password/token/request payload;
- public mapping ke private storage;
- Swagger secret example;
- dependency/lockfile yang berubah tanpa review.

## 9. Changelog dan tag

Perbarui `CHANGELOG.md`, commit final, lalu:

```bash
git tag -a v0.1.0 -m "Rust API Skeleton v0.1.0"
git push origin main
git push origin v0.1.0
```

Tag hanya dibuat setelah seluruh gate yang relevan benar-benar dijalankan dan
hasilnya dicatat di `docs/verification.md`. Warna hijau karena “sepertinya aman”
tidak termasuk hasil test, walaupun manusia punya sejarah panjang mencoba
mengajukan itu sebagai bukti.
