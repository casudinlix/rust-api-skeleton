# Rust API Skeleton

Skeleton backend **API-only** dengan pola Laravel: controller/handler tipis,
service untuk business logic, repository untuk akses data, middleware, job queue,
dan task scheduler. PostgreSQL dan MySQL didukung oleh satu codebase melalui SQLx.

API, worker, dan scheduler berjalan sebagai proses terpisah. Driver queue default
menyimpan job dalam database. Driver Redis memakai Apalis dengan transactional
outbox SQL untuk menjaga perubahan domain dan pekerjaan tetap konsisten.

## Aturan agent dan desain sistem

Agent wajib mengikuti [AGENTS.md](AGENTS.md) dan
[standar desain sistem](docs/system-design-rules.md): clean code, query tanpa N+1,
kolom SELECT eksplisit, cursor pagination, PK `<entitas>_id` dengan UUIDv7, serta
desain query/indeks dan migration sebelum implementasi fitur. Contoh user, queue,
dan generator mengikuti aturan tersebut. Lihat [desain penyesuaian contoh](docs/design/example-rule-alignment.md)
untuk perubahan skema, kontrak API/cursor, bukti indeks, dan upgrade data lama.

## Fitur

- Axum 0.8, Tokio, SQLx 0.8.6, Rust 2024 edition.
- PostgreSQL dan MySQL: default build memuat kedua driver; bisa build satu driver.
- Cache Redis opsional: JSON, TTL, namespace, timeout, dan helper `remember`.
- Queue database/Redis yang dapat dipilih secara independen dari cache; Apalis
  Redis, outbox SQL, relay terkelola/terpisah, dan concurrency worker terbatas.
- Command bergaya Laravel melalui `make` dan binary Rust `artisan`: install,
  server + worker, generator modul, migration, key, dan dependency management.
- Register/login JWT HS256, Argon2id, current-user guard, admin guard.
- CRUD user untuk admin, profile sendiri untuk user, cursor pagination.
- Normalisasi sebelum validasi, JSON envelope, error terpusat, CORS allowlist.
- Request ID, structured tracing, JSON/pretty logs, timeout, body limit,
  per-peer auth rate limit, dan graceful SIGINT/SIGTERM shutdown.
- Listener observability terpisah per process dengan liveness, readiness,
  Prometheus text metrics, DB-pool gauges, serta worker/relay/scheduler counters.
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
make relay # Opsional: relay terpisah untuk QUEUE_DRIVER=redis
make scheduler
```

`cargo run --locked` secara default menjalankan `api`. `src/main.rs` tetap tersedia
sebagai entrypoint kompatibilitas `rust-api-skeleton`; API, worker, relay,
scheduler, dan migrate menggunakan bootstrap library yang sama. `make serve` tidak melakukan
reload otomatis saat
kode berubah; hentikan lalu jalankan ulang setelah mengedit kode. Untuk deployment,
jalankan API/worker/scheduler sebagai service terpisah seperti contoh Docker.

```bash
curl --fail http://127.0.0.1:3000/health
curl --fail http://127.0.0.1:3000/ready
```

Migration tidak berjalan otomatis ketika API/worker/relay/scheduler boot. Jalankan
`migrate` sebagai langkah deployment sebelum menjalankan atau mengganti service.
Binary `migrate` hanya menerapkan migration maju; tidak menyediakan reset database
atau rollback destruktif otomatis.

Lihat [panduan command development](docs/development.md) untuk generator,
penambahan package, rotasi key, dan contoh alur membuat fitur. Untuk module
pertama, gunakan [starter pattern](docs/module-init.md) dan source yang dapat
dikompilasi di [examples/module_init.rs](examples/module_init.rs).

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

Prefix aplikasi: `/api/v1`. Semua response aplikasi menggunakan JSON envelope
standar; HEAD dan preflight OPTIONS mengikuti semantik HTTP tanpa response body.

### Kontrak response standar

Success biasa:

```json
{
  "success": true,
  "message": "User retrieved",
  "data": {
    "user_id": "..."
  }
}
```

Success list dengan cursor:

```json
{
  "success": true,
  "message": "Users retrieved",
  "data": [],
  "meta": {
    "next_cursor": null,
    "has_more": false
  }
}
```

Error:

```json
{
  "success": false,
  "message": "Validation failed",
  "errors": {
    "email": [
      {"code": "email", "message": "must be a valid email address"}
    ]
  }
}
```

`errors` hanya muncul bila ada detail aman, seperti hasil validation. Error umum
tidak mengirim `data: null`. Semua module HTTP memakai helper global
`crate::response`: `response::success`, `response::created`,
`response::success_with_status`, dan `response::cursor`. Seluruh failure
mengalir melalui `AppError`, yang membentuk envelope dengan `response::error`.
Service/domain tetap bebas dari Axum `Json` dan status HTTP.

Request body dibuat dengan `make request NAME=CreateCustomerRequest`. Scaffold
menggunakan `Deserialize + Validate`, `deny_unknown_fields`, dan `Normalize`;
handler memakainya melalui `ValidatedJson` dan service melakukan validasi ulang
dengan `normalize_and_validate`.


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
| GET | `/api/v1/users/{user_id}` | Admin | Detail user |
| PUT | `/api/v1/users/{user_id}` | Admin | Mengubah nama/email user |
| DELETE | `/api/v1/users/{user_id}` | Admin | Menghapus user; self-delete admin ditolak |

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

`limit` default 20, maksimum 100. Respons daftar memakai kontrak
`ApiCursorResponse`; item berada langsung di `data`, sedangkan metadata cursor
berada di `meta`:

```json
{
  "success": true,
  "message": "Users retrieved",
  "data": [],
  "meta": {
    "next_cursor": null,
    "has_more": false
  }
}
```

`has_more` diturunkan dari keberadaan `next_cursor`, bukan dari query count
tambahan. Jika `next_cursor` berisi nilai, kirim nilainya sebagai query `cursor` berikutnya.
Cursor memakai `(created_at, user_id)` descending dan tidak bergantung pada offset
atau total count. Envelope cursor versi 1 terikat resource `users`, sort
`created_at_desc_user_id_desc`, dan scope `admin:all`; maksimum 512 byte encoded.
Context, bentuk UUID, dan presisi timestamp divalidasi sebelum repository dipanggil.
Jangan membuat/mengedit cursor sendiri. Cursor tidak mengenkripsi timestamp/UUID
dan bukan mekanisme otorisasi; endpoint tetap memakai admin guard.

### Identitas user dan upgrade contoh lama

Semua response user memakai field `user_id`, termasuk register, login, profil,
detail, dan item list. PK skema akhir adalah `users.user_id`, `jobs.job_id`, dan
`job_outbox.job_outbox_id`. Semua baris baru dibuat dengan UUIDv7; query publik
hanya mengambil kolom response, sedangkan `password_hash` hanya dibaca untuk login.

Migration `20261102000000_align_example_primary_keys.sql` tersedia untuk kedua
driver. File migration historis tetap immutable: jalankan seluruh history dengan
`make migrate` untuk memperoleh skema akhir. Rename kolom mempertahankan nilai
UUID lama, referensi job, deduplication key, dan subject JWT. Data historis UUIDv4
tetap dapat dibaca; ID lama tidak diganti diam-diam menjadi UUIDv7.

Untuk upgrade deployment existing, hentikan API/worker/scheduler/relay, jalankan
migration, lalu jalankan binary versi baru. Client perlu mengganti pembacaan field
`id` menjadi `user_id` dan mengulang pagination dari halaman pertama karena cursor
lama ditolak. Penambahan kolom generated/index jobs memerlukan maintenance window
yang diukur pada ukuran data Anda. Ikuti [prosedur upgrade](docs/design/example-rule-alignment.md#upgrade-and-rollback-operations)
untuk urutan rilis, kesiapan schema, dan penanganan DDL MySQL yang terputus.

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

`QUEUE_DRIVER=database` adalah default. Registrasi memakai satu SQL transaction
untuk `users` dan `jobs`. Dengan `QUEUE_DRIVER=redis`, transaksi menyimpan `users`
dan `job_outbox`; relay menerbitkan ID job ke Apalis Redis setelah commit. Apabila
salah satu INSERT gagal, keduanya dibatalkan. API tidak mengirim email secara sinkron.

Worker database mengambil job memakai transaction dan `FOR UPDATE SKIP LOCKED`, lalu
melakukan pekerjaan di luar transaction tersebut. Setiap pengambilan menaikkan
attempt dan membuat token lease baru. ACK/retry hanya diterima jika token cocok
dan lease belum expired. Job yang ditinggalkan proses crash dapat diambil ulang;
attempt terakhir yang expired dipindah ke status `failed`.

- Default satu job aktif per proses. `QUEUE_CONCURRENCY=1..64` membatasi
  concurrency; sesuaikan dengan pool database dan kapasitas provider.
- Default job timeout 30 detik, lease 90 detik, maksimum 5 attempts, polling 1 detik.
- Retry menggunakan exponential backoff dengan jitter dan batas maksimum satu jam.
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
TLS database, reverse proxy, dan deployment. Health/metrics process dijelaskan di
[panduan observability](docs/observability.md).

### Mengaktifkan worker Redis

```dotenv
QUEUE_DRIVER=redis
QUEUE_REDIS_URL=redis://127.0.0.1:6380/0
QUEUE_REDIS_PREFIX=my-project:local:queue:v1
QUEUE_REDIS_TIMEOUT_MS=2000
QUEUE_CONCURRENCY=1
QUEUE_RELAY_ENABLED=true
```

```bash
make docker-queue
make migrate
make serve
```

Relay berjalan bersama worker Redis secara default. Untuk service relay terpisah,
set `QUEUE_RELAY_ENABLED=false` pada worker dan jalankan `make relay`. Redis
mendistribusikan delivery; SQL tetap menyimpan payload, lease, retry, dan hasil.
ID aplikasi pada outbox adalah UUIDv7; ID internal Apalis merupakan metadata
transport. Recovery/publish ulang tidak mengubah ID aplikasi.

`CACHE_DRIVER=redis` dan `QUEUE_DRIVER=redis` tidak saling mengaktifkan. Compose
menyediakan Redis queue persisten di port 6380 terpisah dari cache disposable
di port 6379. Mengganti driver hanya mengubah tujuan job baru: tetap jalankan
worker backend lama sampai backlog aktifnya habis. Lihat [panduan queue](docs/queue.md)
untuk peralihan driver, konfigurasi, outbox, recovery, dan pengujian.

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
| Executables | `src/bin/` | `api`, `worker`, `relay`, `scheduler`, `migrate`, `artisan` |

Untuk modul baru, lengkapi `docs/design/<nama_fitur>.md` serta rancangan query,
PK/FK, constraint, dan indeks terlebih dahulu. Buat dan verifikasi migration kedua
database sebelum model/repository/service/handler fitur. Jangan mengedit migration
yang sudah diterapkan; SQLx memeriksa checksum. Integration tests harus menguji
query sebenarnya pada PostgreSQL dan MySQL.

Generator menyediakan titik awal untuk langkah-langkah tersebut:

```bash
make migration NAME=create_customers_table
# Isi dan verifikasi SQL kedua driver sebelum implementasi fitur.
make model NAME=Customer
make request NAME=CreateCustomerRequest
make repository NAME=CustomerRepository
make service NAME=CustomerService
make controller NAME=CustomerController
```

Modul Rust didaftarkan otomatis. Business logic, dependency injection, policy,
route, dan SQL tetap Anda lengkapi secara eksplisit. Generator menolak file/modul
yang sudah ada. Gunakan [starter module](docs/module-init.md) sebagai pola init
untuk request validation dan helper global response, lalu lihat
[panduan development](docs/development.md) untuk detail generator.

Untuk job baru: tetapkan `kind` dan payload ber-versi yang tidak menyimpan secret,
buat handler idempotent, lalu daftarkan dispatch pada lapisan jobs. Gunakan facade
queue dari `AppState` agar driver terpilih tetap dihormati. Apabila job merupakan
bagian atomik dari perubahan domain, masukkan keduanya dalam transaction seperti
contoh registrasi user; jangan melakukan commit domain lalu enqueue terpisah.

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

Untuk queue Redis, set `QUEUE_DRIVER=redis` dan
`DOCKER_QUEUE_REDIS_URL=redis://redis-queue:6379/0`, lalu jalankan
`make docker-queue` sebelum worker. Relay sudah termasuk worker secara default.
Jika relay dikelola terpisah, set `QUEUE_RELAY_ENABLED=false` dan jalankan
`docker compose --profile relay up -d relay`.

Image multi-stage menjalankan binary sebagai UID/GID 10001. Compose app services
memakai filesystem read-only, `/tmp` sementara, drop capabilities, dan
`no-new-privileges`. Port API dan database development hanya di-publish ke loopback.
Default image healthcheck berlaku untuk API; Compose menonaktifkannya pada worker,
scheduler, relay, dan migration.

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
TEST_REDIS_URL='redis://127.0.0.1:6380/0' make test-redis-transport
TEST_DATABASE_URL='postgres://postgres:password@127.0.0.1:5432/postgres' \
  TEST_REDIS_URL='redis://127.0.0.1:6380/0' make test-queue-postgres
TEST_DATABASE_URL='mysql://root:password@127.0.0.1:3306/mysql' \
  TEST_REDIS_URL='redis://127.0.0.1:6380/0' make test-queue-mysql
```

Suite menguji migration, UTC, CRUD, duplicate race, transaction rollback,
pagination dengan timestamp yang sama, HTTP auth/role, multi-worker claims,
expired leases, stale ACK/retry, retry exhaustion, serta outbox/transport Redis.
Hasil live test dicatat pada dokumen verifikasi; ignored bukan lulus. Akun
produksi tidak memerlukan privilege membuat database untuk menjalankan aplikasi.

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
- [Apalis](https://docs.rs/apalis/latest/apalis/)
- [Apalis Redis](https://docs.rs/apalis-redis/latest/apalis_redis/)
