# Rust API Skeleton

Skeleton backend **API-only** dengan pola Laravel: controller/handler tipis,
service untuk business logic, repository untuk akses data, middleware, job queue,
dan task scheduler. PostgreSQL dan MySQL didukung oleh satu codebase melalui SQLx.

API, worker, dan scheduler berjalan sebagai proses terpisah. Queue disimpan dalam
database yang sama, sehingga job tetap tersedia setelah proses restart.

## Aturan agent dan desain sistem

Agent wajib mengikuti [AGENTS.md](AGENTS.md) dan
[standar desain sistem](docs/system-design-rules.md): clean code, query tanpa N+1,
kolom SELECT eksplisit, cursor pagination, PK `<entitas>_id` dengan UUIDv7, serta
desain query/indeks dan migration sebelum implementasi fitur. Dokumen tersebut
juga mencatat penyesuaian legacy yang masih diperlukan pada kode saat ini.

## Fitur

- Axum 0.8, Tokio, SQLx 0.8.6, Rust 2024 edition.
- PostgreSQL dan MySQL: default build memuat kedua driver; bisa build satu driver.
- Cache Redis opsional: JSON, TTL, namespace, timeout, dan helper `remember`.
- Command bergaya Laravel melalui `make` dan binary Rust `artisan`: install,
  server + worker, generator modul, migration, key, dan dependency management.
- Register/login JWT HS256, Argon2id, current-user guard, admin guard.
- CRUD user untuk admin, profile sendiri untuk user, cursor pagination.
- Normalisasi sebelum validasi, JSON envelope, error terpusat, CORS allowlist.
- Request ID, structured tracing, JSON/pretty logs, timeout, body limit,
  per-peer auth rate limit, dan graceful SIGINT/SIGTERM shutdown.
- Transaksi atomik user + welcome job, leased queue claims, retry/backoff,
  stale-worker fencing, dan penyimpanan failed jobs.
- Scheduler UTC dengan deduplikasi maintenance per menit; welcome email via SMTP
  atau transport `log` untuk development.
- `/health`, `/ready`, explicit migration binary, non-root Docker image,
  development Compose, Makefile, dan CI untuk kedua database serta cache Redis.

## 1. Menjalankan secara lokal

### Persiapan

Jika menggunakan paket source ZIP, ekstrak folder `rust-api-skeleton/` dan mulai
dari langkah `make install`; tidak perlu melakukan clone lagi.

Pasang Rust melalui [rustup](https://rustup.rs/) dan GNU Make. File `rust-toolchain.toml`
menetapkan **Rust 1.99.1** beserta rustfmt dan clippy. Gunakan PostgreSQL 16+
atau MySQL 8.0.16+; Compose/CI menyediakan MySQL 8.4. MySQL harus memakai InnoDB
dan mendukung `SKIP LOCKED` serta enforced CHECK constraints. MariaDB belum diuji.

```bash
git clone https://github.com/casudinlix/rust-api-skeleton.git
cd rust-api-skeleton
make install
```

`make install` menyiapkan `.env` dari contoh bila belum ada, menghasilkan
`JWT_SECRET` acak bila masih kosong/placeholder, lalu mengambil dan membangun
dependency sesuai `Cargo.lock`. Konfigurasi dan key yang sudah valid dipertahankan.
Secret tidak dicetak ke terminal. Isi `DATABASE_URL` untuk database yang sudah
dibuat; command ini tidak membuat database atau menjalankan migration.

Untuk database development via Docker:

```bash
# PostgreSQL: default .env.example
make docker-db DB_DRIVER=postgres

# Alternatif MySQL, setelah mengganti DB_DRIVER dan kedua DATABASE_URL di .env:
# make docker-db DB_DRIVER=mysql
```

### Migration, API, dan worker

```bash
make migrate
make serve
```

`make serve` membangun binary lalu menjalankan **API + worker** dalam satu terminal.
Ctrl+C/SIGTERM menghentikan keduanya bersama. Bila satu proses keluar tanpa
diminta, supervisor menghentikan proses lainnya dan mengembalikan status gagal.
Supervisor development ini mendukung Unix (Linux/macOS; gunakan WSL pada Windows).
Untuk menyertakan scheduler:

```bash
make serve SCHEDULER=1
```

Untuk terminal atau process manager terpisah, shortcut lama tetap tersedia:

```bash
make run
make worker
make scheduler
```

`cargo run --locked` secara default menjalankan `api`. `src/main.rs` tetap tersedia
sebagai entrypoint kompatibilitas `rust-api-skeleton`; API, worker, scheduler, dan
migrate menggunakan bootstrap library yang sama. `make serve` tidak melakukan
reload otomatis saat
kode berubah; hentikan lalu jalankan ulang setelah mengedit kode. Untuk deployment,
jalankan API/worker/scheduler sebagai service terpisah seperti contoh Docker.

```bash
curl --fail http://127.0.0.1:3000/health
curl --fail http://127.0.0.1:3000/ready
```

Migration tidak berjalan otomatis ketika API/worker/scheduler boot. Jalankan
`migrate` sebagai langkah deployment sebelum menjalankan atau mengganti service.
Binary `migrate` hanya menerapkan migration maju; tidak menyediakan reset database
atau rollback destruktif otomatis.

Lihat [panduan command development](docs/development.md) untuk generator,
penambahan package, rotasi key, dan contoh alur membuat fitur.

## 2. Memilih database

| Mode | Build | Runtime |
|---|---|---|
| Kedua driver | `cargo build --locked --release --bins` | `DB_DRIVER=postgres` atau `mysql` |
| PostgreSQL saja | `cargo build --locked --release --no-default-features --features postgres --bins` | `DB_DRIVER=postgres` |
| MySQL saja | `cargo build --locked --release --no-default-features --features mysql --bins` | `DB_DRIVER=mysql` |

**PostgreSQL:**

```dotenv
DB_DRIVER=postgres
DATABASE_URL=postgres://rust_api:local-development-only@127.0.0.1:5432/rust_api
DOCKER_DATABASE_URL=postgres://rust_api:local-development-only@postgres:5432/rust_api
```

**MySQL:**

```dotenv
DB_DRIVER=mysql
DATABASE_URL=mysql://rust_api:local-development-only@127.0.0.1:3306/rust_api
DOCKER_DATABASE_URL=mysql://rust_api:local-development-only@mysql:3306/rust_api
```

`DATABASE_URL` digunakan oleh binary lokal; Compose memakai `DOCKER_DATABASE_URL`
untuk nama host di jaringan container. Driver, scheme URL, dan Cargo feature wajib
cocok. Memilih driver yang tidak dikompilasi menghasilkan error saat startup.

Ini adalah pemilihan **satu database per proses**, bukan koneksi simultan ke dua
database atau sinkronisasi data otomatis. Mengganti driver tidak memindahkan data.
Jalankan migration pada database tujuan dan rencanakan migrasi data tersendiri.

SQLx menggunakan typed `PgPool`/`MySqlPool`, bukan `AnyPool`. PostgreSQL menyimpan
UUID native dan TIMESTAMPTZ; MySQL menyimpan UUID `CHAR(36)` dan `DATETIME(6)`.
Adapter MySQL mengonversi timestamp ke UTC. Semua koneksi mengatur session UTC.
SQL selalu memakai bind parameters. Build tidak membutuhkan database hidup;
query dialect diverifikasi melalui integration tests pada database nyata.

## 3. Endpoint API

Prefix aplikasi: `/api/v1`. Semua response aplikasi menggunakan JSON envelope;
HEAD dan preflight OPTIONS mengikuti semantik HTTP tanpa response body.

| Method | Endpoint | Akses | Fungsi |
|---|---|---|---|
| GET | `/health` | Public | Process liveness |
| GET | `/ready` | Public | Koneksi database + migration/checksum yang diperlukan |
| POST | `/api/v1/auth/register` | Public, rate limited | Registrasi user + welcome job + JWT |
| POST | `/api/v1/auth/login` | Public, rate limited | Login dan penerbitan JWT |
| GET | `/api/v1/me` | Bearer | Profile sendiri |
| PUT | `/api/v1/me` | Bearer | Mengubah nama/email sendiri |
| GET | `/api/v1/users` | Admin | Daftar user dengan cursor |
| POST | `/api/v1/users` | Admin | Membuat user tanpa menerbitkan token impersonasi |
| GET | `/api/v1/users/{id}` | Admin | Detail user |
| PUT | `/api/v1/users/{id}` | Admin | Mengubah nama/email user |
| DELETE | `/api/v1/users/{id}` | Admin | Menghapus user; self-delete admin ditolak |

### Register dan login

```bash
curl -i http://127.0.0.1:3000/api/v1/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"name":"Administrator","email":"admin@example.com","password":"use-a-unique-long-password"}'

curl -i http://127.0.0.1:3000/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"use-a-unique-long-password"}'
```

Registrasi selalu menghasilkan role `user`. Field tak dikenal seperti `role`
ditolak, bukan diteruskan ke model. Provision admin pertama secara sadar melalui
database administratif; jangan menambahkan default admin password ke image:

```sql
UPDATE users SET role = 'admin' WHERE email = 'admin@example.com';
```

Gunakan `data.access_token` pada response login/register:

```bash
curl http://127.0.0.1:3000/api/v1/me \
  -H "Authorization: Bearer $API_TOKEN"

curl 'http://127.0.0.1:3000/api/v1/users?limit=20' \
  -H "Authorization: Bearer $API_TOKEN"
```

`API_TOKEN` adalah variabel lokal yang Anda isi dengan token hasil login. Token
berlaku default 900 detik. Issuer, audience, signature, expiry, not-before,
issued-at dan UUID subject diverifikasi; toleransi waktu diatur nol. Sinkronkan
jam host. Role tidak disalin ke JWT: guard membaca user/role terbaru dari database.
User yang dihapus langsung kehilangan akses walaupun token belum expired.

Skeleton ini menyediakan access token. Refresh token, password reset, email
verification, MFA, dan token-revocation store belum menjadi fitur. Logout client
menghapus token lokal; token yang dicuri tetap berlaku sampai expiry kecuali
account dihapus atau signing key dirotasi.

### Cursor pagination

`limit` default 20, maksimum 100. Respons daftar:

```json
{
  "success": true,
  "message": "Users retrieved",
  "data": {
    "items": [],
    "next_cursor": null
  }
}
```

Jika `next_cursor` berisi nilai, kirim nilainya sebagai query `cursor` berikutnya.
Cursor memakai `(created_at, id)` descending dan tidak bergantung pada offset
atau total count. Jangan membuat/mengedit cursor sendiri. Cursor tidak mengenkripsi
timestamp/UUID dan bukan mekanisme otorisasi; endpoint tetap memakai admin guard.

### Validasi dan error

Nama dipangkas dan dibatasi 1–100 karakter. Email dipangkas dan diubah menjadi
lowercase sebelum validasi/unique check. Ini adalah kebijakan email
aplikasi; email internationalized belum didukung sebagai kebutuhan khusus.
Password tidak dipangkas atau diubah: registrasi menerima 12–128 byte, kemudian
menyimpannya sebagai Argon2id PHC string dengan salt acak. Nilai password/hash
tidak pernah menjadi bagian response atau validation metadata.

| HTTP | Makna |
|---|---|
| 400 | JSON, query, cursor, atau parameter path tidak valid |
| 401 / 403 | Kredensial tidak valid / akses ditolak |
| 404 / 409 | Resource tidak ada / konflik unique atau operasi |
| 413 / 415 | Body terlalu besar / Content-Type tidak didukung |
| 422 | DTO gagal validasi |
| 429 | Batas request autentikasi per peer terlampaui |
| 503 | Readiness/dependency unavailable atau kapasitas hashing sedang penuh |
| 500 | Error internal dengan pesan publik yang disanitasi |

## 4. Queue, worker, dan scheduler

Registrasi memakai satu SQL transaction untuk `users` dan `jobs`. Apabila salah
satu INSERT gagal, keduanya dibatalkan. API tidak mengirim email secara sinkron.

Worker mengambil job memakai transaction dan `FOR UPDATE SKIP LOCKED`, lalu
melakukan pekerjaan di luar transaction tersebut. Setiap pengambilan menaikkan
attempt dan membuat token lease baru. ACK/retry hanya diterima jika token cocok
dan lease belum expired. Job yang ditinggalkan proses crash dapat diambil ulang;
attempt terakhir yang expired dipindah ke status `failed`.

- Satu worker process mengeksekusi satu job pada satu waktu. Skalakan dengan
  beberapa proses/container worker; tidak ada queue in-memory lintas proses.
- Default job timeout 30 detik, lease 90 detik, maksimum 5 attempts, polling 1 detik.
- Retry menggunakan exponential backoff dengan batas maksimum satu jam.
- Delivery bersifat **at least once**. Job handler perlu idempotent; keberhasilan
  SMTP yang diikuti crash sebelum ACK tetap bisa menyebabkan email terkirim ulang.
- SIGTERM/SIGINT menghentikan pengambilan baru dan memberi kesempatan job aktif
  selesai sampai deadline. Lease menyediakan recovery apabila proses terhenti paksa.
- `MAIL_TRANSPORT=log` hanya mensimulasikan pengiriman dan tetap menyelesaikan job.
  `MAIL_TRANSPORT=smtp` menggunakan TLS/STARTTLS dan kredensial dari environment.
  `APP_ENV=production` menolak transport log agar email tidak diam-diam tidak terkirim.
- Account yang sudah dihapus tidak dikirimi welcome email; job tersebut selesai
  sebagai no-op.

Scheduler default berjalan setiap 5 menit dalam UTC dan mengantrekan maintenance
untuk membersihkan completed jobs yang lebih tua dari 7 hari. Failed jobs tetap
tersimpan untuk inspeksi. Trigger dalam menit UTC yang sama memakai unique key,
sehingga beberapa scheduler tidak mengantrekan maintenance yang sama dua kali.
Retensi completed jobs juga menentukan lama penyimpanan kunci deduplikasi.

Cron memiliki field detik, contohnya `0 */5 * * * *`. Scheduler ini tidak melakukan
catch-up atas jadwal yang terlewat ketika seluruh scheduler mati. Maintenance
didesain aman untuk dilewati dan diulang; kebutuhan business schedule dengan
catch-up memerlukan pencatatan occurrence tersendiri.

Lihat [Panduan operasi](docs/operations.md) untuk status queue, retry manual,
TLS database, reverse proxy, dan deployment.

**Apakah worker bisa memakai Redis?** Bisa, sebagai cache di handler atau sebagai
backend queue yang diimplementasikan tersendiri. Worker bawaan saat ini tetap
memakai `DatabaseQueue`. `CACHE_DRIVER=redis` mengaktifkan fasilitas cache dan
tidak memindahkan job ke Redis. Rancangan queue Redis yang mempertahankan transaksi
user + job dijelaskan dalam [panduan cache dan Redis worker](docs/cache.md#redis-untuk-worker).

## 5. Struktur kode dan cara menambah modul

| Lapisan | Lokasi | Tanggung jawab |
|---|---|---|
| Configuration | `src/config/` | Env, driver, validasi setting |
| Database | `src/database/` | Pool, UTC session, ping, migration |
| Cache | `src/cache/` | Facade JSON, backend Redis/none, TTL, fallback |
| Domain | `src/domain/models/`, `src/domain/dtos/` | Model internal dan payload API |
| Repository | `src/repositories/` | Trait + adapter PostgreSQL/MySQL |
| Service | `src/services/` | Business logic, auth, pagination |
| Handler | `src/handlers/` | Extract input, call service, JSON response |
| Route | `src/routes/api.rs` | Mapping endpoint seperti Laravel routes/api.php |
| Middleware | `src/middleware/` | Auth guard, CORS, deadline, rate limit |
| Jobs | `src/jobs/` | Queue durability dan job handlers |
| Scheduler | `src/schedulers/task_scheduler.rs` | UTC cron mengantrekan job |
| Bootstrap/state | `src/bootstrap.rs`, `src/state.rs` | Dependency wiring dan process lifecycle |
| Developer tools | `src/console/` | Bootstrap project, generator, dependency commands, supervisor |
| Executables | `src/bin/` | `api`, `worker`, `scheduler`, `migrate`, `artisan` |

Untuk modul baru: buat model/DTO dan service, definisikan trait repository,
implementasikan kedua dialect, kemudian daftarkan handler/route. Buat migration
dengan versi yang sama di kedua direktori database. Jangan mengedit migration
yang sudah diterapkan; SQLx memeriksa checksum. Integration tests harus menguji
query sebenarnya pada PostgreSQL dan MySQL.

Generator menyediakan titik awal untuk langkah-langkah tersebut:

```bash
make controller NAME=CustomerController
make model NAME=Customer
make request NAME=CreateCustomerRequest
make service NAME=CustomerService
make repository NAME=CustomerRepository
make migration NAME=create_customers_table
```

Modul Rust didaftarkan otomatis. Business logic, dependency injection, policy,
route, dan SQL tetap Anda lengkapi secara eksplisit. Generator menolak file/modul
yang sudah ada; lihat [panduan development](docs/development.md) untuk detail.

Untuk job baru: tetapkan `kind` dan payload ber-versi yang tidak menyimpan secret,
buat handler idempotent, lalu daftarkan dispatch di `src/jobs/worker.rs`. Gunakan
`DatabaseQueue::enqueue` dengan deduplication key bila perlu. Apabila job merupakan
bagian atomik dari perubahan domain, masukkan keduanya dalam transaction seperti
contoh registrasi user.

## 6. Docker

```bash
# .env sudah diisi, terutama JWT_SECRET dan DOCKER_DATABASE_URL.
make docker-build
make docker-db DB_DRIVER=postgres
docker compose run --rm migrate
make docker-up
docker compose logs -f api worker scheduler
```

Untuk MySQL, ubah setting driver/URL dan gunakan `make docker-db DB_DRIVER=mysql`.
Build image khusus satu driver dengan `make docker-build DB_FEATURES=mysql`.

Image multi-stage menjalankan binary sebagai UID/GID 10001. Compose app services
memakai filesystem read-only, `/tmp` sementara, drop capabilities, dan
`no-new-privileges`. Port API dan database development hanya di-publish ke loopback.
Default image healthcheck berlaku untuk API; Compose menonaktifkannya pada worker,
scheduler, dan migration.

Container aplikasi boleh memakai database eksternal: isi `DOCKER_DATABASE_URL`
dengan host yang dapat dijangkau container, lalu cukup jalankan migration dan
service aplikasi. Tidak perlu menyalakan service database Compose.

## 7. Pengujian

```bash
make check
```

Perintah tersebut menjalankan format check, clippy dengan warning sebagai error,
unit/HTTP tests dan contoh rustdoc, lalu compile PostgreSQL-only dan MySQL-only.
`Cargo.lock` disertakan dan build/test memakai `--locked`. Perintah `make require`,
`make remove`, dan `make update` sengaja dapat mengubah manifest/lockfile.

Untuk verifikasi generator dari pembuatan file sampai compile hasilnya, jalankan
`make test-scaffolds`. Suite ini memakai salinan project sementara dan ditandai
ignored karena menjalankan Cargo lagi pada output generator; tidak memerlukan
database dan tidak dijalankan otomatis di dalam `make check`.

Integration suites ditandai ignored supaya unit tests tidak membutuhkan DB.
Jalankan secara eksplisit dengan **database disposable**, dan akun test yang
memiliki izin membuat/menghapus database untuk isolasi queue tests:

```bash
TEST_DATABASE_URL='postgres://postgres:password@127.0.0.1:5432/rust_api_test' make test-postgres
TEST_DATABASE_URL='mysql://root:password@127.0.0.1:3306/rust_api_test' make test-mysql
TEST_REDIS_URL='redis://127.0.0.1:6379/0' make test-redis
```

Suite menguji migration, UTC, CRUD, duplicate race, transaction rollback,
pagination dengan timestamp yang sama, HTTP auth/role, multi-worker claims,
expired leases, stale ACK/retry, retry exhaustion, dan deduplikasi. Akun produksi
tidak memerlukan privilege membuat database untuk menjalankan aplikasi.

Redis tests memakai namespace acak dan menghapus key miliknya sendiri, tanpa
`FLUSHDB`/`FLUSHALL`. Gunakan endpoint plaintext `redis://` pada instance disposable;
akun test membutuhkan `ACL SETUSER`/`ACL DELUSER` untuk satu user test bernama acak,
dan proses test perlu dapat membuka listener loopback. Hak tersebut tidak
diperlukan oleh akun aplikasi produksi. Live Redis tests ditandai ignored dan
dijalankan secara eksplisit oleh `make test-redis` serta job CI integrasi.

Detail hasil yang benar-benar dijalankan tersedia di
[verification.md](docs/verification.md). Workflow CI dikonfigurasi untuk PostgreSQL
16, MySQL 8.4, dan Redis 8.0. Verifikasi lokal menjadi acuan penyelesaian perubahan.
Status merah GitHub Actions juga dapat berasal dari kuota/runner yang tidak
tersedia; periksa log penyebabnya, jangan menyimpulkan kode gagal hanya dari badge.

## 8. Cache Redis

Cache default adalah `none`, sehingga aplikasi tidak membutuhkan Redis. Untuk
mengaktifkannya pada aplikasi lokal, isi `.env`:

```dotenv
CACHE_DRIVER=redis
REDIS_URL=redis://127.0.0.1:6379/0
CACHE_PREFIX=rust-api-skeleton:cache:
CACHE_DEFAULT_TTL_SECS=300
CACHE_TIMEOUT_MS=500
CACHE_MAX_VALUE_BYTES=1048576
```

Jalankan `make docker-cache` untuk menyediakan Redis development. Aplikasi dalam
Compose memakai `DOCKER_REDIS_URL=redis://redis:6379/0`; ganti URL tersebut jika
Redis ada di VM eksternal. Profile Redis memakai cache disposable dengan batas
memori 128 MiB dan eviction `allkeys-lru`.

Service dapat menerima `state.cache.clone()` dan menggunakan `get`, `put`,
`forget`, atau `remember`. `remember` menjalankan loader ketika key tidak ada
atau Redis bermasalah, lalu mencoba menyimpan hasil yang berhasil. Semua write
memakai TTL. Autentikasi dan pembacaan user bawaan tetap menggunakan database;
caching data bisnis ditambahkan secara eksplisit oleh service.

Lihat [docs/cache.md](docs/cache.md) untuk contoh Rust, invalidation, TLS,
batas konfigurasi, dan perbedaan cache dengan queue Redis.

## Referensi stack

- [Axum 0.8](https://docs.rs/axum/0.8.9/axum/)
- [SQLx 0.8.6 features](https://docs.rs/crate/sqlx/0.8.6/features)
- [SQLx MySQL type mapping](https://docs.rs/sqlx-mysql/0.8.6/sqlx_mysql/types/index.html)
- [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)
- [PostgreSQL locking SELECT](https://www.postgresql.org/docs/current/sql-select.html)
- [MySQL locking reads](https://dev.mysql.com/doc/refman/8.4/en/innodb-locking-reads.html)
- [tokio-cron-scheduler](https://docs.rs/tokio-cron-scheduler/0.15.1/tokio_cron_scheduler/)
- [redis-rs](https://docs.rs/redis/1.8.1/redis/)
