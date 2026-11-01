# Queue database, Redis, dan transactional outbox

Skeleton menyediakan dua driver queue: **`database`** sebagai default dan
**`redis`** melalui Apalis. Binary worker tetap aplikasi Rust/Tokio. Cache dan
queue mempunyai konfigurasi independen; `CACHE_DRIVER=redis` tidak memilih
driver queue.

## Memilih driver

| Driver | Catatan pekerjaan | Pengambilan pekerjaan |
|---|---|---|
| `database` | Tabel SQL `jobs` | Worker mengambil lease melalui `SKIP LOCKED` |
| `redis` | Tabel SQL `job_outbox` | Relay menerbitkan ID ke Apalis Redis; worker mengambil lease SQL untuk ID yang diterima |

Mode Redis memakai SQL sebagai sumber durabilitas, status, retry, dan hasil.
Redis membagi delivery kepada worker. Worker Redis tetap membutuhkan database
untuk membaca payload domain, memvalidasi lease, mencatat hasil, serta menjalankan
service/repository. Rancangan ini tidak menghilangkan semua query/write SQL.

### Database

```dotenv
QUEUE_DRIVER=database
QUEUE_CONCURRENCY=1
```

```bash
make migrate
make serve
```

Redis tidak diperlukan bila cache juga `none`. Worker mengambil pekerjaan melalui
transaksi singkat dan menjalankan handler setelah transaksi claim selesai.

### Redis

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
# Tambahkan scheduler bila diperlukan:
# make serve SCHEDULER=1
```

`make docker-queue` menyediakan Redis development terpisah di port **6380**.
Service cache dari `make docker-cache` tetap di port **6379**. Di dalam Compose,
set `DOCKER_QUEUE_REDIS_URL=redis://redis-queue:6379/0`. Untuk Redis eksternal,
gunakan URL yang dapat dijangkau oleh masing-masing proses/container.

Semua API, worker, scheduler, dan relay untuk satu antrean harus memakai database,
prefix, serta kebijakan timeout/retry yang konsisten. Prefix harus unik per
project, environment, dan database aplikasi. Worker yang terhubung ke database
berbeda tidak boleh berbagi prefix antrean yang sama: mereka dapat mengambil ID
job milik database lain sehingga delivery tertunda. Outbox tidak menyimpan
prefix tujuan per baris. Perubahan prefix harus dikoordinasikan dengan drain
backlog dan cleanup namespace lama; jangan menggantinya pada sebagian worker
ketika job masih aktif.

## Konfigurasi

| Setting | Default | Kontrak |
|---|---|---|
| `QUEUE_DRIVER` | `database` | `database` atau `redis`; memilih tujuan job baru |
| `QUEUE_CONCURRENCY` | `1` | 1–64 job aktif per worker process |
| `QUEUE_POLL_INTERVAL_MS` | `1000` | 100–60000 ms; jeda polling pekerjaan/relay |
| `QUEUE_VISIBILITY_TIMEOUT_SECS` | `90` | Lease maksimal 3600 detik; minimal 10 detik lebih lama daripada job timeout |
| `QUEUE_JOB_TIMEOUT_SECS` | `30` | 1–300 detik per eksekusi handler |
| `QUEUE_MAX_ATTEMPTS` | `5` | 1–100 claim eksekusi; publish Redis tidak menghabiskan attempt bisnis |
| `QUEUE_RETRY_BASE_SECS` | `5` | 1–3600 detik; dasar retry/backoff |
| `QUEUE_REDIS_URL` | kosong | Wajib untuk Redis; URL terpisah dari `REDIS_URL` milik cache |
| `QUEUE_REDIS_PREFIX` | `rust-api-skeleton:queue:v1` | 1–96 byte ASCII huruf/angka, `:`, `_`, `-`; harus konsisten antara publisher dan consumer |
| `QUEUE_REDIS_TIMEOUT_MS` | `2000` | 100–30000 ms untuk operasi transport Redis, lebih pendek dari visibility timeout |
| `QUEUE_RELAY_ENABLED` | `true` | Menjalankan relay bersama worker Redis |

Gunakan `rediss://` untuk TLS dengan certificate server yang dipercaya. Credential
ACL harus di-URL-encode. Query parameter/fragment dan opsi bypass verifikasi TLS
ditolak. URL/password tidak boleh dicetak ke log atau dimasukkan ke git.

Adapter ini menggunakan endpoint Redis standalone melalui `redis://` atau
`rediss://`. Redis Cluster dan discovery Sentinel belum diekspos oleh konfigurasi
ini; dukungan tersebut memerlukan adapter dan pengujian tersendiri.

## Transaksi domain dan outbox

Pada driver database, pembuatan user dan welcome job tetap tersimpan bersama dalam
transaksi `users` + `jobs`. Pada driver Redis, transaksi yang sama menyimpan
`users` + `job_outbox`. API tidak memanggil Redis saat memegang transaksi domain.

Jika Redis tidak tersedia, catatan outbox tetap tersimpan dan relay dapat mencoba
lagi. Keberhasilan request berarti perubahan domain serta pekerjaan sudah
tercatat; tidak berarti email atau side effect eksternal sudah selesai.

Alur Redis:

1. Transaksi SQL menyimpan outbox berstatus `pending`.
2. Relay mengambil reservasi publish yang dibatasi lease dan mengirim envelope
   berisi versi serta ID aplikasi ke Apalis Redis.
3. Worker menerima envelope, lalu mengambil lease eksekusi berdasarkan PK SQL.
   Kind, payload, scope, serta status berasal dari SQL, bukan dari Redis.
4. Handler berhasil: SQL berubah menjadi `completed`. Handler gagal: SQL menjadi
   `pending` dengan backoff, atau `failed` sesuai kategori/batas percobaan.
5. Catatan aktif yang tidak selesai akan dipublish ulang setelah jatuh tempo.
   Redis yang kehilangan data atau delivery duplikat dapat dipulihkan dari SQL.

`job_outbox.job_outbox_id` adalah UUIDv7 dan juga `job_id` pada envelope. ID itu
tetap sama sepanjang retry. ULID internal Apalis merupakan metadata transport,
bukan pengganti PK aplikasi. Backend Apalis memakai struktur antrean internalnya
sendiri; konfigurasi ini bukan driver Redis Streams.

Versi Apalis dipin, dengan patch lokal pada `apalis-redis` untuk membatasi jumlah
scheduled delivery yang dipindahkan dan jumlah reservasi worker mati yang
dipulihkan dalam satu eksekusi Lua. Reservasi yang belum dipulihkan tetap
tersimpan untuk batch berikutnya. Source patch ikut dibawa ke build lokal dan
image Docker. Catatan perubahan dan cara meninjau upgrade tersedia di
[PATCHES.md](../vendor/apalis-redis/PATCHES.md).

Lease publish dan eksekusi memakai token berbeda. Transition hanya diterima jika
token dan lease masih sah. Delivery duplikat yang tidak mendapatkan lease SQL
tidak menjalankan handler. Record `completed` atau `failed` tidak dijalankan lagi
hanya karena envelope lama muncul kembali.

Delivery tetap **at least once**. Jika SMTP/provider berhasil tetapi proses mati
sebelum status sukses dicatat, efek eksternal dapat terulang. Gunakan idempotency
key provider dan constraint/transaksi domain bila tersedia. Deduplication key
enqueue bukan jaminan exactly-once efek eksternal.

Lihat [desain query, indeks, dan state machine](design/redis-queue.md) untuk
kontrak lengkap. Payload aplikasi dibatasi 64 KiB; simpan referensi file besar
alih-alih memasukkan isi file ke job.

## Relay terpisah dan scaling

Untuk development, relay sudah dikelola dalam worker Redis sehingga `make serve`
tetap menjalankan API dan worker tanpa proses tambahan yang harus dioperasikan.
Untuk deployment yang memisahkan relay:

```dotenv
QUEUE_DRIVER=redis
QUEUE_RELAY_ENABLED=false
```

Jalankan masing-masing sebagai service tersendiri:

```bash
make worker
make relay
make scheduler
```

Binary `relay` berjalan sebagai relay khusus, terlepas dari flag bundling worker.
Untuk container, aktifkan service opsional:

```bash
docker compose --profile relay up -d relay
```

Beberapa relay boleh berjalan untuk availability; lease SQL membatasi kepemilikan
publish. Pilih concurrency worker berdasarkan koneksi database, rate limit
provider, CPU, dan memori. Job yang panjang perlu dipecah menjadi pekerjaan
terbatas; lease eksekusi saat ini tidak diperpanjang dengan heartbeat.

### Graceful shutdown

SIGINT/SIGTERM menghentikan pengambilan pekerjaan baru dan memberi waktu untuk
menyelesaikan job aktif. Untuk driver Redis, `SHUTDOWN_TIMEOUT_SECS` harus lebih
besar dari `QUEUE_JOB_TIMEOUT_SECS` ditambah
`ceil(4 × QUEUE_REDIS_TIMEOUT_MS / 1000)`: anggaran ini mencakup ACK terakhir,
penutupan Apalis, dan dua operasi cleanup. Contoh default 30 detik untuk handler
dan 2000 ms per operasi Redis memerlukan grace lebih dari 38 detik; default
45 detik memenuhi batas tersebut.

Perhitungkan juga waktu tunggu/koneksi database dan request API; anggaran Redis
ini bukan jaminan bahwa SQL yang tersendat akan selesai. Deadline shutdown luar
tetap membatasi keseluruhan drain dan penutupan pool. Jika deadline habis, lease
SQL menjaga pekerjaan agar dapat dipulihkan setelah kedaluwarsa. Atur grace
penghentian container/orchestrator agar lebih panjang daripada deadline aplikasi.

## Pergantian driver tanpa meninggalkan backlog

Mengganti `QUEUE_DRIVER` memengaruhi **job baru**. Perubahan itu tidak menyalin,
menghapus, atau mengonversi backlog di backend sebelumnya.

### Database ke Redis

1. Terapkan migration baru dalam deployment terkoordinasi dan siapkan Redis queue.
2. Jalankan API/scheduler/worker baru dengan `QUEUE_DRIVER=redis`.
3. Pertahankan worker database tambahan selama job lama masih `pending/running`:

```bash
QUEUE_DRIVER=database make worker
# Atau override pada binary worker tanpa mengganti environment API:
cargo run --locked --bin worker -- --driver database
```

4. Hentikan worker database tambahan setelah backlog aktifnya habis. Tinjau
   failed jobs secara terpisah; failed bukan pekerjaan yang otomatis dipindahkan.

### Redis ke database

Arahkan API/scheduler dan worker utama ke `database`, tetapi pertahankan worker
Redis beserta relay sampai outbox aktif habis:

```bash
QUEUE_DRIVER=redis QUEUE_RELAY_ENABLED=true make worker
```

Tetap sediakan `QUEUE_REDIS_URL`/prefix yang sama dengan backlog Redis sebelumnya.
Jangan menghapus volume atau key queue ketika masih ada pekerjaan aktif. Jangan
mengubah `kind`, payload, ID, atau deduplication key untuk memaksa perpindahan
backend. Migrasi backlog eksplisit memerlukan rancangan tersendiri.

## Operasional dan recovery

Compose `redis-queue` menyimpan data di volume `redis_queue_data`, memakai AOF
`everysec`, batas memori 256 MiB, dan `noeviction`. Ini konfigurasi development:
pasang autentikasi/ACL, jaringan privat, kapasitas, backup, serta recovery yang
sesuai pada deployment nyata. AOF `everysec` masih dapat kehilangan penulisan
terbaru ketika terjadi crash. Active outbox dipertahankan untuk republish, tetapi
idempotensi handler tetap diperlukan.

`noeviction` menolak write tertentu ketika kapasitas habis; relay harus tetap
mempertahankan catatan SQL dan melaporkan kegagalan. Jangan memakai fallback cache
untuk menganggap publish gagal sebagai sukses. Logical database `/0` dan `/1`
pada satu instance tetap berbagi memori dan kebijakan eviction.

Monitor backlog SQL, umur pekerjaan tertua, attempts, error category, restart
worker/relay, Redis memory/persistence, serta pool database. `/ready` adalah
readiness API terhadap database/schema; endpoint ini tidak membuktikan keberadaan
worker, kesehatan Redis queue, atau keberhasilan pengiriman email. Detail query
diagnosis dan retry tersedia di [panduan operasi](operations.md#pemeriksaan-queue).

Retensi completed jobs dikendalikan maintenance dengan maksimum 1.000 baris per
panggilan pada driver database maupun Redis/outbox. Seleksi PK memakai urutan
`updated_at` dan PK, indeks retention, serta `SKIP LOCKED`; penghapusan batch ada
dalam transaksi yang sama. Failed jobs tetap tersedia untuk inspeksi di SQL.
Consumer membersihkan metadata terminal Apalis secara otomatis
dalam batch terbatas; cleanup dapat tertunda ketika Redis tidak tersedia. Source
audit status/hasil tetap SQL, sehingga metadata delivery terminal tidak perlu
disimpan sebagai riwayat bisnis kedua.

Apalis juga memakai indeks metadata internal `core:apalis:queues:list` selain
key data task yang memakai namespace. Sesuaikan ACL dengan operasi/key library
yang benar-benar dipakai dan uji pada deployment; jangan menganggap seluruh key
library selalu diawali prefix aplikasi. Jangan melakukan `FLUSHDB`/`FLUSHALL`
untuk membersihkan antrean, terutama pada instance yang digunakan aplikasi lain.

## Pengujian

Gunakan database dan Redis disposable. Suite database membuat database terisolasi;
akun test memerlukan izin CREATE/DROP DATABASE. Namespace Redis dibuat unik dan
cleanup hanya menghapus key milik test.

```bash
TEST_DATABASE_URL='postgres://postgres:password@127.0.0.1:5432/postgres' \
  make test-postgres
TEST_DATABASE_URL='mysql://root:password@127.0.0.1:3306/mysql' \
  make test-mysql

TEST_REDIS_URL='redis://127.0.0.1:6380/0' make test-redis-transport

TEST_DATABASE_URL='postgres://postgres:password@127.0.0.1:5432/postgres' \
  TEST_REDIS_URL='redis://127.0.0.1:6380/0' make test-queue-postgres
TEST_DATABASE_URL='mysql://root:password@127.0.0.1:3306/mysql' \
  TEST_REDIS_URL='redis://127.0.0.1:6380/0' make test-queue-mysql
```

Test integrasi ditandai ignored dan harus dijalankan secara eksplisit. Lihat
[hasil verifikasi](verification.md) untuk perintah yang benar-benar sudah
dijalankan; adanya target Make atau konfigurasi CI tidak membuktikan test lulus.
