# Command development bergaya Laravel

`make` menyediakan shortcut yang familier; implementasinya berada dalam binary
Rust `artisan`. Perintah setup dan generator dapat digunakan tanpa database atau
Redis yang sedang hidup. Jalankan command dari direktori project yang berisi
`Cargo.toml` dan `Makefile`.

## Mulai dari project baru

```bash
git clone https://github.com/casudinlix/rust-api-skeleton.git
cd rust-api-skeleton
make install
```

Siapkan Rust melalui [rustup](https://rustup.rs/) dan GNU Make lebih dahulu.
Toolchain mengikuti `rust-toolchain.toml`. Instalasi pertama membutuhkan koneksi
untuk mengunduh dependency yang belum ada di cache Cargo.

`make install`:

1. Membuat `.env` dari `.env.example` jika belum ada.
2. Mengisi `JWT_SECRET` yang hilang, kosong, atau masih placeholder dengan
   32 byte acak dari sistem operasi, ditulis sebagai 64 karakter hexadecimal.
3. Mengambil dan membangun dependency sesuai `Cargo.lock`.

Konfigurasi `.env` serta key valid yang sudah ada dipertahankan. Secret tidak
dicetak; file `.env` ditulis secara atomik dengan izin `0600` pada Unix. Key custom
yang tidak memenuhi syarat tidak diam-diam diganti: perbaiki konfigurasi atau
gunakan rotasi key secara eksplisit. `make setup` adalah alias `make install`.

Referensi secret eksternal seperti `JWT_SECRET=${MY_DEPLOYMENT_SECRET}` juga
dipertahankan. Pastikan sumber environment tersebut tersedia saat aplikasi
berjalan. Sintaks dotenv yang rusak, assignment `JWT_SECRET` ganda, atau secret
multiline ditolak agar command tidak salah menulis konfigurasi; perbaiki berkas
secara eksplisit sebelum mencoba kembali.

Isi driver dan URL database dalam `.env`. Untuk PostgreSQL development bawaan:

```bash
make docker-db DB_DRIVER=postgres
make migrate
make serve
```

Untuk MySQL, ubah `DB_DRIVER=mysql`, `DATABASE_URL`, dan `DOCKER_DATABASE_URL`
terlebih dahulu, lalu jalankan `make docker-db DB_DRIVER=mysql`. Parameter
`DB_DRIVER` pada `docker-db` memilih service Compose; konfigurasi aplikasi tetap
harus sesuai. `make install` tidak membuat database dan tidak menerapkan migration.

## Daftar command

Jalankan `make help` untuk daftar singkat.

| Command | Kegunaan |
|---|---|
| `make install` | Siapkan environment, key awal, dan dependency terkunci |
| `make setup` | Alias `install` |
| `make serve` | Bangun dan jalankan API + worker dalam satu terminal |
| `make serve SCHEDULER=1` | Jalankan API + worker + scheduler |
| `make run` | Jalankan API saja |
| `make worker` | Jalankan durable queue worker saja |
| `make relay` | Jalankan relay outbox terpisah untuk driver Redis |
| `make scheduler` | Jalankan scheduler saja |
| `make migrate` | Terapkan migration untuk driver yang dipilih |
| `make controller NAME=CustomerController` | Buat controller/handler JSON |
| `make model NAME=Customer` | Buat model domain |
| `make request NAME=CreateCustomerRequest` | Buat DTO request dengan normalisasi/validasi |
| `make dto NAME=CreateCustomerRequest` | Alias generator request |
| `make service NAME=CustomerService` | Buat service business logic |
| `make repository NAME=CustomerRepository` | Buat trait + adapter PostgreSQL dan MySQL |
| `make job NAME=SendInvoiceJob` | Buat payload dan handler job |
| `make middleware NAME=AuditMiddleware` | Buat scaffold middleware |
| `make migration NAME=create_customers_table` | Buat pasangan file migration kedua dialect |
| `make key` | Buat key jika belum ada/masih placeholder |
| `make key FORCE=1` | Rotasi signing key yang sudah valid |
| `make require PACKAGE=csv@1` | Tambah dependency melalui `cargo add` |
| `make require PACKAGE=serde@1 FEATURES=derive` | Tambah/ubah dependency beserta features |
| `make require PACKAGE=pretty_assertions@1 DEV=1` | Tambah dev-dependency |
| `make remove PACKAGE=csv` | Hapus dependency melalui `cargo remove` |
| `make remove PACKAGE=pretty_assertions DEV=1` | Hapus dev-dependency |
| `make update PACKAGE=redis` | Perbarui lockfile untuk package yang dipilih |
| `make update` | Perbarui seluruh dependency dalam batas manifest |
| `make fmt` | Format kode Rust |
| `make check` | Format check, clippy, tests, dan compile kedua mode driver |
| `make docker-queue` | Jalankan Redis queue development persisten pada port 6380 |
| `make test-redis-transport` | Uji transport queue dengan `TEST_REDIS_URL` disposable |
| `make test-queue-postgres` | Uji worker/outbox Redis dengan PostgreSQL dan kedua URL test |
| `make test-queue-mysql` | Uji worker/outbox Redis dengan MySQL dan kedua URL test |
| `make test-scaffolds` | Buat semua scaffold pada project sementara dan verifikasi compile |
| `make build DB_FEATURES=mysql` | Build release dengan driver MySQL saja |

`NAME` berisi satu nama Rust sederhana seperti `CustomerController` atau
`customer_controller`; jangan gunakan path, namespace PHP, atau potongan kode.
Gunakan nama ASCII yang diawali huruf, maksimum 100 karakter, dan bukan keyword Rust.
Generator memvalidasi nama, mendeteksi benturan setelah normalisasi, dan menolak
file atau deklarasi modul yang sudah ada. Tidak ada opsi overwrite otomatis.

| Contoh generator | Berkas utama yang dibuat |
|---|---|
| `controller NAME=CustomerController` | `src/handlers/customer_handler.rs` |
| `model NAME=Customer` | `src/domain/models/customer.rs` |
| `request NAME=CreateCustomerRequest` | `src/domain/dtos/create_customer_request.rs` |
| `service NAME=CustomerService` | `src/services/customer_service.rs` |
| `repository NAME=CustomerRepository` | `src/repositories/customer_repository.rs`, `src/repositories/postgres/customer_pg_repo.rs`, `src/repositories/mysql/customer_mysql_repo.rs` |
| `job NAME=SendInvoiceJob` | `src/jobs/send_invoice.rs` |
| `middleware NAME=AuditMiddleware` | `src/middleware/audit.rs` |
| `migration NAME=create_customers_table` | `migrations/{postgres,mysql}/<versi>_create_customers_table.sql` |

Suffix umum boleh disertakan atau dihilangkan: `Customer`, `CustomerController`,
dan `CustomerHandler` merujuk scaffold controller yang sama. Karena itu, nama
kedua yang menghasilkan berkas/modul sama ditolak sebagai benturan.

`PACKAGE` menerima satu nama crate dengan versi opsional, misalnya `csv@1`.
`FEATURES` berisi daftar feature dipisahkan koma. Command dependency meneruskan
argumen langsung ke Cargo; tidak menjalankan input melalui shell. Sesudah
menambahkan, menghapus, atau memperbarui dependency, tinjau perubahan
`Cargo.toml`/`Cargo.lock` dan jalankan `make check`. Menghapus crate yang masih
digunakan source akan membuat build gagal sampai pemakaiannya diperbarui.

## Menjalankan API bersama worker

```bash
make serve
# atau
make serve SCHEDULER=1 SHUTDOWN_TIMEOUT=60
```

Supervisor membangun binary yang diperlukan sekali, lalu menjalankan executable
API, worker, dan scheduler opsional sebagai proses terpisah. Output masing-masing
proses tampil di terminal yang sama. Lokasi executable mengikuti hasil Cargo,
termasuk bila `CARGO_TARGET_DIR` diubah.

Ctrl+C atau SIGTERM meminta semua proses berhenti dengan baik. Jika salah satu
proses keluar sendiri, supervisor menghentikan sibling-nya dan melaporkan gagal;
terminal tidak dibiarkan seolah seluruh stack masih berjalan. `SHUTDOWN_TIMEOUT`
adalah waktu tenggang supervisor dalam detik, default 60. Setelah batas habis,
process group yang tersisa menerima SIGKILL; supervisor menyediakan maksimum
5 detik tambahan untuk menunggu proses langsung selesai. SIGTERM berulang tetap
menjalankan shutdown tertib dengan deadline semula; Make dapat meneruskan sinyal
yang sudah diterima supervisor. Ctrl+C saat shutdown berlangsung melewati sisa
waktu tenggang dan langsung melakukan pembersihan paksa. Pilih waktu tenggang
lebih panjang daripada `SHUTDOWN_TIMEOUT_SECS` aplikasi (default 45 detik) agar
pekerjaan aktif sempat diselesaikan. Process group digunakan pada Unix untuk
menghentikan proses turunan, termasuk ketika proses induknya sudah keluar.

`make serve` mendukung Linux/macOS; pada Windows gunakan lingkungan Unix seperti
WSL. Command ini tidak memulai database/Redis, tidak menjalankan migration, dan
tidak menyediakan file watcher atau hot reload. Jika source berubah, hentikan
lalu jalankan kembali. Untuk production, gunakan service/container terpisah
seperti [panduan operasi](operations.md) dan Compose yang sudah tersedia.

Worker mengikuti `QUEUE_DRIVER=database|redis`. Driver Redis membutuhkan
`QUEUE_REDIS_URL` dan menjalankan relay outbox di dalam worker secara default;
`make serve` tetap mengelola API + worker. Untuk memisahkan relay, set
`QUEUE_RELAY_ENABLED=false`, lalu jalankan `make relay` sebagai proses tersendiri.
`CACHE_DRIVER=redis` hanya mengatur cache dan tidak memilih backend queue.

Untuk mencoba Redis queue secara lokal:

```bash
make docker-queue
# Isi .env: QUEUE_DRIVER=redis dan QUEUE_REDIS_URL=redis://127.0.0.1:6380/0
make migrate
make serve
```

Pergantian driver hanya mengubah job baru. Jalankan
`QUEUE_DRIVER=database make worker` untuk menguras job SQL lama setelah memilih
Redis; arah sebaliknya tetap membutuhkan worker Redis + relay sampai outbox aktif
habis. Lihat [panduan queue](queue.md) sebelum mengganti driver pada project yang
sudah mempunyai backlog.

## Alur membuat fitur baru

Tulis desain fitur dan rancang PK/FK, query, cursor, constraint, serta indeks
terlebih dahulu sesuai [aturan sistem](system-design-rules.md). Contoh berikut
menyiapkan migration dan berkas untuk fitur customer:

```bash
make migration NAME=create_customers_table
# Lengkapi dan verifikasi migration kedua database sebelum implementasi fitur.
make model NAME=Customer
make request NAME=CreateCustomerRequest
make repository NAME=CustomerRepository
make service NAME=CustomerService
make controller NAME=CustomerController
```

Generator menambahkan deklarasi modul pada `mod.rs` yang sesuai sambil
mempertahankan import, export, dan implementasi yang sudah ada. Hasil scaffold
dibuat agar dapat dikompilasi. Method bisnis yang belum diimplementasikan
mengembalikan error eksplisit; scaffold bukan CRUD yang sudah selesai.

Lengkapi fitur secara berurutan:

1. Lengkapi `docs/design/<nama_fitur>.md`, migration PostgreSQL/MySQL, serta
   verifikasi indeks dan kandidat query pada data disposable yang representatif.
2. Definisikan field model dan payload request. Normalisasikan text sebelum
   validasi, tolak field tak dikenal, dan jangan normalisasikan password.
3. Lengkapi kontrak repository serta query pada **kedua** adapter database.
   Gunakan bind parameter untuk semua nilai dari pengguna.
4. Pasang repository/service melalui dependency injection di `AppState` atau
   komponen bootstrap yang relevan. Service berisi business logic dan policy.
5. Hubungkan handler dengan service, tetapkan policy akses, lalu daftarkan route
   secara eksplisit di `src/routes/api.rs`.
6. Jalankan `make check` serta kedua suite database untuk perubahan persistence.

Generator tidak memasang route, dependency injection, atau akses publik secara
otomatis. Untuk job baru:

```bash
make job NAME=SendInvoiceJob
```

Lengkapi handler yang idempotent, versi payload, dan dispatch di
lapisan jobs. Gunakan facade queue untuk dispatch standalone dan transaction
ketika job harus atomik dengan perubahan domain. Driver Redis mencatat outbox
pada transaction tersebut; jangan publish Redis sebelum commit. Membuat file job
belum membuat job tersebut dapat dikonsumsi worker.

Untuk middleware:

```bash
make middleware NAME=AuditMiddleware
```

Scaffold middleware perlu dilengkapi lalu dipasang ke router secara eksplisit.
Handler awalnya menolak request sampai policy diimplementasikan; jangan memasang
stub ke route aplikasi sebelum melengkapinya. Generator tidak menentukan
authentication atau authorization policy untuk Anda.

### Migration selalu berpasangan

`make migration NAME=create_customers_table` membuat file dengan versi yang sama
di `migrations/postgres/` dan `migrations/mysql/`. Versi dinaikkan bila terdapat
benturan waktu agar pemanggilan cepat dengan deskripsi berbeda tetap memiliki
versi unik. Nama/deskripsi migration yang sudah ada ditolak.

File awal hanya berisi komentar/petunjuk: **isi SQL kedua dialect sebelum
menjalankan `make migrate`**. Generator tidak menebak tabel, kolom, index, atau
foreign key dari nama migration. Migration kosong yang sudah diterapkan akan
memiliki checksum; memperbaikinya setelah diterapkan memerlukan migration baru.
Jangan mengedit migration yang sudah berjalan pada database bersama/production.

## Key dan environment

```bash
make key
# Rotasi sadar terhadap key yang sudah ada:
make key FORCE=1
```

`make key` menolak mengganti key valid tanpa `FORCE=1`. Rotasi mengubah `.env`;
restart semua API process yang memakai key tersebut agar konfigurasi baru aktif.
Token yang ditandatangani key lama akan ditolak setelah pergantian key. Untuk
deployment yang memasok secret melalui environment/secret manager, perbarui
sumber secret deployment tersebut juga; environment proses mengungguli `.env`.

Bootstrap dan generator menolak destination symlink. Generator menolak file
scaffold yang sudah ada; rotasi key menggunakan opsi `FORCE=1` yang eksplisit.
`.env` tidak pernah dimasukkan ke Git. Jangan menyalin secret development ke image
atau commit.

## Menggunakan Artisan langsung

Semua command baru dapat dipanggil tanpa Make:

```bash
cargo run --locked --bin artisan -- --help
cargo run --locked --bin artisan -- install
cargo run --locked --bin artisan -- serve --scheduler
cargo run --locked --bin artisan -- make:controller CustomerController
cargo run --locked --bin artisan -- require csv@1
cargo run --locked --bin artisan -- key:generate --force
```

CLI menerima `--root /path/project` jika lokasi project perlu ditentukan secara
eksplisit. Contoh setelah binary selesai dibangun:

```bash
./target/debug/artisan --root /path/rust-api-skeleton make:service BillingService
```

Image production hanya memuat binary runtime API, worker, relay, scheduler, dan migrate.
Artisan adalah alat development; tidak diperlukan oleh service yang sudah
dideploy.

## Verifikasi lokal dan GitHub CI

Gunakan `make check` untuk kualitas lokal, lalu jalankan suite integrasi yang
relevan pada service disposable. Detail perintah dan kebutuhan akun tersedia di
[README](../README.md#7-pengujian). Status GitHub Actions bersifat tambahan:
kegagalan akibat kuota/runner tidak menjadi patokan kualitas kode. Jika workflow
gagal, baca penyebabnya dan bedakan dengan kegagalan build atau test yang nyata.

Verifikasi menyeluruh terhadap hasil generator tersedia sebagai langkah terpisah:

```bash
make test-scaffolds
# Padanan Cargo:
cargo test --locked --test artisan_scaffolds -- --ignored --test-threads=1
```

Suite tersebut membuat seluruh jenis scaffold pada salinan project sementara,
lalu memverifikasi bahwa hasilnya dapat dikompilasi. Test ditandai ignored supaya
`make check` tidak menjalankan build Cargo bersarang secara otomatis. Tidak ada
database atau Redis yang diperlukan untuk pengujian ini.
