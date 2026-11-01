# Panduan operasi

## Proses dan database

Jalankan `api`, `worker`, dan `scheduler` sebagai service terpisah. Mereka memakai
schema dan signing key yang sama. Jalankan `migrate` sekali pada fase deployment
sebelum service aplikasi. Migration SQLx memiliki checksum dan lock; jangan
mengubah file migration yang sudah diterapkan. Tambahkan migration baru untuk
perubahan berikutnya. MySQL DDL tidak memiliki rollback transaction penuh;
backup dan uji migration sebelum deployment data nyata.

`DB_MAX_CONNECTIONS` adalah batas **per proses**, bukan batas seluruh deployment.
Contoh: dua API, tiga worker, dan satu scheduler dengan nilai 10 dapat membuka
hingga 60 koneksi, ditambah proses migration/administratif. Sesuaikan dengan
kapasitas database atau pooler dan jumlah replica.

Timestamp aplikasi dan queue memakai UTC. PostgreSQL memakai TIMESTAMPTZ; MySQL
DATETIME tidak menyimpan timezone sehingga adapter dan session selalu memakai
UTC. Konversi Asia/Jakarta dilakukan saat menampilkan data di client.

Untuk produksi, buat database lebih dahulu, gunakan user migration terpisah
dengan hak DDL, dan user aplikasi dengan hak minimum CRUD pada schema. SQLite dan
MariaDB bukan target kompatibilitas yang diuji.

## TLS, secret, dan reverse proxy

Termination HTTPS dilakukan oleh reverse proxy/load balancer. API listen pada
alamat/port terkonfigurasi dan tidak mengelola certificate HTTPS sendiri.

Gunakan secret acak yang sama pada semua API replica. Jangan simpan `.env`, DSN
produksi atau SMTP credential dalam git/image. `JWT_SECRET` HS256 harus minimal
32 byte dan sebaiknya dihasilkan dengan `openssl rand -hex 32`. Rotasi signing
key membatalkan seluruh access token lama; dukungan beberapa key/JWKS belum ada.

Untuk database di jaringan tidak tepercaya gunakan TLS dengan verifikasi server.
SQLx dibangun dengan rustls. Contoh parameter DSN:

```text
postgres://USER:PASSWORD@db.example:5432/app?sslmode=verify-full&sslrootcert=/run/secrets/db-ca.pem
mysql://USER:PASSWORD@db.example:3306/app?ssl-mode=VERIFY_IDENTITY&ssl-ca=/run/secrets/db-ca.pem
```

URL-encode karakter khusus dalam user/password dan mount CA read-only bila
memakai private CA. Verifikasi parameter sesuai server melalui
[PG options](https://docs.rs/sqlx/0.8.6/sqlx/postgres/struct.PgConnectOptions.html)
dan [MySQL options](https://docs.rs/sqlx/0.8.6/sqlx/mysql/struct.MySqlConnectOptions.html).

CORS memakai origin eksplisit; wildcard dan credential cookie lintas-origin
tidak diaktifkan. Bearer token dikirim pada Authorization header.

Rate limiter aplikasi untuk register/login menggunakan alamat peer socket,
tidak mempercayai `X-Forwarded-For`. Di belakang reverse proxy semua client dapat
terlihat sebagai satu peer. Terapkan rate limiting per client pada proxy/gateway
dengan konfigurasi trusted-proxy yang benar, kemudian sesuaikan
`AUTH_REQUESTS_PER_MINUTE` sebagai batas agregat per proses. Batas ini tidak
dibagi antar replica. Batas concurrency hashing tetap berlaku untuk melindungi
memori dan thread saat request dibatalkan.

Batas body, timeout request, dan slot hashing bukan batas global jumlah koneksi.
Konfigurasikan connection limit, header/idle timeout, batas HTTP/2 streams, dan
kapasitas upstream pada reverse proxy sesuai workload. Skeleton tidak memasang
limiter koneksi terdistribusi atau global process-wide connection cap.

Registrasi publik merupakan contoh default. Bila produk Anda hanya menyediakan
akun undangan atau provisioning internal, hapus route public register atau
tambahkan policy bisnis sebelum deployment.

## Health dan observability

- `/health`: liveness proses, tidak bergantung pada database.
- `/ready`: koneksi database serta migration/checksum yang dibutuhkan binary;
  database belum dimigrasikan menghasilkan 503. Endpoint ini tidak memeriksa
  SMTP, Redis cache opsional, atau keberadaan worker, dan tidak menggantikan
  pemeriksaan schema manual.
- Logs JSON default, `LOG_FORMAT=pretty` untuk lokal, `RUST_LOG` untuk level.
- Setiap response membawa `x-request-id`; span mencatat method, path dan ID.
  Query string, Authorization, password, DTO body, dan email recipient tidak
  dicatat oleh middleware bawaan. SQL statement logging dinonaktifkan.
- Job logs menyertakan ID/attempt dan kategori error, tanpa payload atau error
  SMTP mentah. Failed jobs tetap tersedia dalam tabel untuk investigasi.

Readiness menggunakan kebijakan ketat: daftar migration, success flag, dan
checksum harus persis cocok dengan binary. Migration versi yang lebih baru juga
membuat binary lama unready. Rencanakan urutan deployment/migration sesuai aturan
ini; zero-downtime rolling migration membutuhkan kebijakan kompatibilitas schema
yang sengaja dirancang lebih luas. User database aplikasi memerlukan SELECT pada
`_sqlx_migrations`. Perubahan DDL manual yang tidak tercatat di ledger tidak
dideteksi oleh pemeriksaan checksum ini.

Pantau jumlah pending/running/failed, umur job pending tertua, DB pool saturation,
latency, response 5xx/429, serta restart worker. Integrasi OTLP/metrics backend
belum dipasang; tracing dapat diteruskan dari stdout melalui collector deployment.

## Redis cache

`CACHE_DRIVER=none` adalah default. `CACHE_DRIVER=redis` membutuhkan URL Redis
valid, tetapi koneksi dibuat saat operasi cache pertama. Redis yang tidak tersedia
tidak mengubah readiness database; operasi eksplisit mengembalikan error dan
`remember` memakai loader sebagai fallback. Pantau Redis sendiri bila cache
dibutuhkan untuk memenuhi target latency aplikasi.

Gunakan `REDIS_URL` untuk binary lokal dan `DOCKER_REDIS_URL` untuk aplikasi di
Compose. Redis dapat berada di VM eksternal. Untuk TLS gunakan `rediss://` dengan
certificate server yang dipercaya; query parameters/fragment termasuk bypass
TLS ditolak. URL/password tidak boleh masuk log atau git. Instance development
dari profile Compose `redis` tidak memakai autentikasi, dibatasi ke port loopback,
dan tidak menyimpan snapshot/AOF. Konfigurasikan autentikasi serta jaringan secara
terpisah untuk deployment nyata.

Cache hanya untuk data yang dapat dibangun kembali. Tetapkan prefix berbeda
untuk proyek/environment, sertakan tenant pada key bisnis, dan hapus key terkait
setelah commit perubahan data. Cache tidak menggantikan pemeriksaan permission.
Ketika Redis timeout, status write/delete mungkin belum diketahui; konsistensi
kuat memerlukan rancangan di lapisan domain, bukan hanya TTL.

Reconnect jaringan memakai connection manager yang sama. Kegagalan handshake
pertama dapat dicoba ulang, tetapi error handshake non-I/O pada reconnect setelah
koneksi pernah berhasil dapat dipertahankan redis-rs 1.8.1. Setelah perubahan
kredensial/ACL atau perbaikan handshake reconnect (`AUTH`/ACL/`SELECT`), perbarui
konfigurasi dan restart proses aplikasi untuk membuat manager baru. Selama cache
gagal, `remember` tetap menggunakan loader.

Backend membaca value melalui script Lua kecil yang memeriksa ukuran di Redis
sebelum mengirim payload. Jika memakai ACL terbatas, izinkan operasi yang dipakai
driver (`EVAL`, `STRLEN`, `GET`, `SET`, `DEL`, `PING`) beserta handshake redis-rs
seperti `AUTH`, `SELECT`, dan `CLIENT SETINFO` sesuai konfigurasi. Batasi key
pattern ke prefix aplikasi dan uji reconnect dengan ACL tersebut. Driver tidak
menyediakan perintah flush global.

Worker bawaan tetap menggunakan queue SQL. Jika queue Redis ditambahkan, gunakan
instance Redis tersendiri dari cache: eviction dan batas memori berlaku per
instance, sehingga pemisahan nomor logical database saja tidak mengisolasinya.
Lihat [panduan cache](cache.md) untuk API penggunaan dan rancangan outbox/worker.

## Pemeriksaan queue

Query berikut berlaku untuk kedua engine:

```sql
SELECT status, COUNT(*) AS total
FROM jobs GROUP BY status;

SELECT id, kind, status, attempts, available_at, locked_until,
       last_error, created_at, updated_at
FROM jobs
WHERE status IN ('pending', 'running', 'failed')
ORDER BY created_at
LIMIT 100;
```

Jangan mengubah token/lease job yang sedang running. Lease default 90 detik dan
job timeout 30 detik; konfigurasi menolak lease yang terlalu pendek. Tidak ada
heartbeat lease: pecah pekerjaan lebih panjang menjadi beberapa job atau tambah
implementasi renewal bertoken bila dibutuhkan.

Semua worker untuk queue yang sama harus memakai `QUEUE_MAX_ATTEMPTS` dan retry
policy yang sama. Policy ini dibaca dari konfigurasi worker; tidak disimpan per
job. Runtime saat ini memiliki satu queue umum dan concurrency satu per process.
Prioritas, beberapa nama queue, dan dashboard queue belum diimplementasikan.

### Retry manual failed job

Periksa penyebab dan pastikan side effect aman diulang. Retry hanya ID job yang
dipilih dan hanya ketika statusnya `failed`. Contoh PostgreSQL:

```sql
UPDATE jobs
SET status = 'pending', attempts = 0, available_at = clock_timestamp(),
    lock_token = NULL, locked_until = NULL, last_error = NULL,
    updated_at = clock_timestamp()
WHERE id = 'PUT-FAILED-JOB-UUID-HERE' AND status = 'failed';
```

Contoh MySQL:

```sql
UPDATE jobs
SET status = 'pending', attempts = 0, available_at = UTC_TIMESTAMP(6),
    lock_token = NULL, locked_until = NULL, last_error = NULL,
    updated_at = UTC_TIMESTAMP(6)
WHERE id = 'PUT-FAILED-JOB-UUID-HERE' AND status = 'failed';
```

Periksa affected rows = 1. Jangan menjalankan UPDATE massal tanpa meninjau side
effect. Deduplikasi enqueue mencegah insertion dengan key yang sama selama row
masih disimpan; deduplikasi ini tidak menjamin exactly-once SMTP delivery.

## Mail dan scheduler

`MAIL_TRANSPORT=log` adalah simulasi lokal; job dianggap selesai dan tidak akan
otomatis dikirim setelah transport diubah ke SMTP. Gunakan SMTP sebelum menerima
registrasi produksi. Port 465 memakai implicit TLS, port lain wajib STARTTLS.
Server certificate diverifikasi. `MAIL_FROM` harus sesuai domain yang diizinkan
provider SMTP. SPF/DKIM/DMARC dan deliverability merupakan konfigurasi domain/provider.

Scheduler mengantrekan maintenance paling banyak sekali per menit UTC, walaupun
cron disetel lebih sering. Deduplikasi menggunakan nama task dan bucket menit
waktu trigger. Sinkronkan jam semua host. Maintenance menghapus completed jobs
berumur lebih dari `SCHEDULER_RETENTION_DAYS`; failed jobs tidak dihapus. Untuk
task dengan cadence per detik, definisikan kunci slot tersendiri.

Jadwal yang terlewat ketika scheduler mati tidak diputar ulang. Tidak ada SLA
bahwa cron berjalan tepat pada detik tertentu. Untuk billing/invoice yang harus
mengejar jadwal terlewat, simpan due occurrences dan enqueue berdasarkan keadaan
domain, lalu gunakan unique key per invoice/periode.

## Shutdown dan kapasitas

API berhenti menerima request baru ketika menerima SIGTERM/SIGINT. Worker berhenti
mengambil job dan menyelesaikan job aktif sampai timeout; scheduler menghentikan
cron, membatalkan penantian enqueue aktif, dan menunggu callback berhenti. Hasil
commit enqueue yang dibatalkan mungkin belum diketahui; unique key melindungi
retry pada slot menit yang sama. Batas drain default 45 detik; Compose memberikan
60 detik sebelum force-stop. Set grace orchestrator lebih panjang daripada
`SHUTDOWN_TIMEOUT_SECS`.

Batas aplikasi mencakup async drain dan penutupan connection pool dalam satu
deadline. Task Argon2 yang sudah masuk `spawn_blocking` tidak dapat dibatalkan
oleh Tokio; runtime dapat menunggu task tersebut selesai ketika keluar. Jumlah
task ini dibatasi semaphore. Force-stop orchestrator tetap menjadi batas akhir
proses apabila host atau pekerjaan blocking tidak merespons.

Argon2id menggunakan 19 MiB memori per operasi dan default empat slot hashing.
Hitung kebutuhan memori hashing bersama runtime, connection pools, request bodies,
dan logging. Tidak ada angka RPS atau sizing VM produksi yang diklaim: ukur
latency dan pemakaian CPU/RAM dengan workload aplikasi dan database yang nyata.

Pada deploy rolling, pastikan payload job dan schema kompatibel dengan worker
versi sebelumnya yang masih hidup. Tambahkan version pada payload jika format
job berubah. Proses schema change yang memutus kompatibilitas memerlukan drain
worker dan rencana deployment tersendiri.
