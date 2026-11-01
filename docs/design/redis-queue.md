# Desain data: queue Redis dan transactional outbox

Baseline: `171b842`. Dokumen dan pasangan migration dibuat sebelum kode fitur.
Queue database tetap default; Redis adalah transport tambahan yang menggunakan
Apalis. Worker tetap proses Rust/Tokio terpisah. Cache dan queue memiliki
konfigurasi serta koneksi independen.

## Use case, skala, dan batas

- API mencatat perubahan domain dan pekerjaan terkait dalam transaksi SQL yang
  sama; scheduler juga dapat mengantrekan pekerjaan.
- Mode database memakai tabel `jobs` lama. Mode Redis memakai `job_outbox` sebagai
  catatan pengiriman dan hasil eksekusi yang tahan kehilangan/restart Redis.
- Relay mengirim envelope kecil berisi versi payload dan UUID aplikasi. Payload
  domain tetap di SQL, dibaca worker hanya setelah berhasil mengambil lease.
- Envelope Redis tidak dipercaya sebagai sumber nama job, payload, tenant, atau
  izin; worker memakai ID untuk mengambil catatan SQL yang sah.
- Anggaran payload aplikasi maksimum 64 KiB; nama kind maksimum 64 ASCII dan
  deduplication key maksimum 191 ASCII. Identitas tenant/policy berada dalam
  payload domain tervalidasi, bukan namespace Redis yang dikirim client.
- Satu reservasi relay dan satu reservasi eksekusi mengambil maksimum satu job.
  Sampai 32 job dengan percobaan habis dapat diterminalkan per pemanggilan relay;
  loop tidak berjalan tanpa batas. Prune completed maksimum 1.000 baris/panggilan.
- Skala fixture desain: 50.000 catatan; campuran completed, delayed pending,
  ready pending, dan expired running. Ini pemeriksaan bentuk plan, bukan janji
  throughput produksi atau pengganti load test terhadap workload deployment.

## Skema dan integritas

Tabel `job_outbox` memakai nama entitas eksplisit `job_outbox` dan PK
`job_outbox_id`: UUIDv7 hasil `Uuid::now_v7()`. Nilai ini juga merupakan `job_id`
aplikasi pada envelope transport. Tidak ada identifier aplikasi kedua yang harus
disinkronkan. ULID internal Apalis hanya metadata transport, tidak mengganti PK.

PostgreSQL menggunakan UUID/JSONB/TIMESTAMPTZ. MySQL menggunakan UUID canonical
CHAR(36) ascii/ascii_bin, JSON, dan DATETIME(6) dengan session UTC. Payload tidak
memiliki FK langsung ke user karena job dapat mereferensikan domain berbeda;
handler memvalidasi keberadaan dan policy domain. Penghapusan user tidak boleh
membuat catatan pekerjaan hilang diam-diam.

| Kolom | Kontrak |
|---|---|
| `job_outbox_id` | PK UUIDv7; identitas aplikasi stabil sepanjang retry/publish ulang |
| `kind`, `payload` | Nama handler dan data domain berversi sesuai handler; maksimum 64 KiB |
| `deduplication_key` | UNIQUE nullable; dipertahankan sampai retensi completed; failed tidak dipangkas otomatis |
| `status` | `pending`, `running`, `completed`, `failed` |
| `attempts` | Jumlah claim eksekusi, bukan jumlah publish Redis; tidak negatif |
| `available_at` | Batas waktu paling awal eksekusi, dipakai retry/backoff |
| `locked_until`, `lock_token` | Lease dan fencing token eksekusi; wajib ada tepat ketika running |
| `relay_due_at` | Waktu paling awal publish/publish ulang; NULL pada terminal |
| `relay_token` | Fencing token reservasi relay; berbeda dari token eksekusi |
| `last_error` | Kategori error aman maksimum 64 ASCII; tidak menyimpan pesan eksternal mentah |
| `created_at`, `updated_at` | UTC; updated_at berubah pada transisi bisnis/eksekusi, bukan heartbeat publish |

Constraint menjaga pasangan lease dengan status, status terminal dengan
`relay_due_at = NULL`, serta relay tidak dijadwalkan sebelum lease eksekusi
berakhir. Token lease menggunakan UUID acak; aturan versi 7 berlaku pada PK.

## State machine dan jaminan

1. **Enqueue:** transaksi menyimpan outbox pending dengan due sekarang. Redis
   tidak dihubungi dari transaksi domain. INSERT domain memakai constraint
   dedupe secara ketat; conflict menggagalkan transaksi. Enqueue standalone
   memperlakukan dedupe yang sudah ada sebagai successful no-op.
2. **Reserve relay:** transaksi singkat mengambil due row dengan SKIP LOCKED,
   memasang relay token baru, dan memajukan due sebesar visibility timeout.
   Tidak ada network Redis saat lock SQL masih dipegang.
3. **Publish sukses:** token yang masih berlaku dapat menghapus relay token dan
   menjadwalkan pemeriksaan/publish ulang setelah visibility timeout. Status
   bisnis tetap pending; ACK publish bukan bukti job telah dijalankan.
4. **Publish gagal:** token yang masih berlaku menjadwalkan ulang setelah
   retry_base_secs. Percobaan bisnis tidak berubah. Crash sebelum/ketika publish
   dipulihkan ketika relay_due_at jatuh tempo. Publish yang hasilnya tidak pasti
   boleh menghasilkan duplikat transport.
5. **Claim worker:** worker Redis mengambil job berdasarkan PK; SQL memeriksa
   status pending yang sudah available atau running yang lease-nya kedaluwarsa.
   Point lookup memakai `FOR UPDATE` dan menunggu lock singkat milik publisher;
   `SKIP LOCKED` hanya dipakai untuk scan kandidat relay. Menganggap lock publish
   sebagai duplicate akan menghapus hint sebelum ada worker yang memilikinya.
   Seluruh klaim, termasuk acquire/commit, dibatasi satu detik. Timeout atau
   pembatalan menutup koneksi klaim, bukan mengembalikan query tertunda ke pool.
   Hasil commit yang tidak pasti tetap dipulihkan melalui lease SQL.
   Claim menaikkan attempts, mengganti execution token, membuang relay token,
   dan menunda relay sampai lease baru berakhir. Publish completion lama tidak
   dapat menimpa jadwal ini karena token relay sudah berubah/dihapus.
6. **Complete/fail:** hanya token eksekusi saat ini dengan lease belum habis
   boleh mengubah catatan. Sukses menjadi completed. Error transient menjadi
   pending dengan backoff; invalid payload/unsupported kind menjadi failed.
   Percobaan terakhir yang mati menjadi failed saat recovery; tidak dipublish
   tanpa akhir. Terminal menghapus relay_due_at sehingga tidak diambil relay.
7. **Duplicate delivery:** worker yang gagal mengambil lease tidak menjalankan
   handler. Delivery transport itu boleh di-ACK; SQL tetap menjadwalkan ulang
   pekerjaan aktif yang kehilangan pemilik atau yang hilang dari Redis.

Outbox adalah sumber kebenaran durabilitas dan hasil, Redis membagi delivery
kepada worker. Desain ini tetap melakukan lookup/transisi SQL per job; tidak
menjanjikan worker bebas database atau eliminasi seluruh write SQL. Jika Redis
kehilangan antrean, active outbox akan dipublish ulang setelah due. Delivery
tetap at-least-once: efek eksternal yang berhasil tepat sebelum crash dapat
terulang, sehingga idempotency domain/provider tetap wajib.

## Pola query dan indeks

| Operasi | Kolom hasil | Predicate/sort | Query budget | Indeks |
|---|---|---|---|---|
| Reserve relay | PK, attempts | relay_due_at <= DB now; ORDER BY relay_due_at, PK; LIMIT 1 SKIP LOCKED | SELECT + UPDATE + commit; maksimum 32 terminal recovery | `(relay_due_at, job_outbox_id)` |
| Publish transition | Tidak ada payload | PK + relay_token + lease/due valid + status aktif | 1 UPDATE | PK |
| Claim execution | PK, kind, payload, attempts | PK; predicate available/lease DB time; FOR UPDATE | SELECT + UPDATE + commit, deadline 1 detik | PK |
| Complete/fail | Tidak ada payload | PK + lock_token + running + lease valid | 1 UPDATE | PK |
| Dedupe enqueue | Tidak ada payload | deduplication_key unik | 1 INSERT | UNIQUE deduplication_key |
| Prune | PK | completed + updated_at cutoff; ORDER BY updated_at, PK; LIMIT 1000 | SELECT PK + bounded DELETE | `(status, updated_at, job_outbox_id)` |
| Welcome recipient | `id AS user_id`, `name`, `email` | PK user dari payload SQL | 1 point lookup | PK users existing |

Handler email memakai read DTO `UserContact`; credential, role, dan timestamp
tidak dimuat untuk pengiriman email. PK users existing sudah melayani point
lookup ini sehingga tidak diperlukan indeks tambahan. Retensi queue SQL legacy
tetap memakai query existing. Perubahan menjadi ordered batch memerlukan desain
indeks/migration tersendiri: fixture 50.000 baris menunjukkan sorting/full scan
pada varian legacy yang hanya menambahkan ORDER BY dan LIMIT.

Relay bukan pagination endpoint; setiap reservasi mengubah due sehingga batch
berikutnya bergerak pada range due tanpa OFFSET. Maintenance memakai bounded
ordered batch dan tidak memuat seluruh tabel. Query count konstan per claim,
bukan pemuatan relasi per item. Tidak ada query `SELECT *`/`RETURNING *`.

Indeks relay PostgreSQL partial `WHERE relay_due_at IS NOT NULL`, sehingga
completed/failed tidak membesarkan indeks aktif. MySQL memakai range indeks
komposit biasa. Equality status dan range updated_at mendahului tie-breaker
pada indeks retensi. PK sudah mencakup claim/ACK; tidak ditambah indeks token
yang tidak dibutuhkan query. Kontrak dedupe tidak memerlukan indeks tambahan.

Kandidat PostgreSQL:

```sql
SELECT job_outbox_id, attempts
FROM job_outbox
WHERE relay_due_at <= statement_timestamp()
ORDER BY relay_due_at, job_outbox_id
LIMIT 1 FOR UPDATE SKIP LOCKED;
```

Kandidat MySQL:

```sql
SELECT job_outbox_id, attempts
FROM job_outbox
WHERE relay_due_at <= UTC_TIMESTAMP(6)
ORDER BY relay_due_at, job_outbox_id
LIMIT 1 FOR UPDATE SKIP LOCKED;
```

Predicate range PostgreSQL memakai `statement_timestamp()` yang stabil dalam
statement agar planner dapat membentuk Index Cond, termasuk ketika seluruh
backlog masih delayed. Guard penyelesaian lease memakai waktu aktual database.
Nilai bind operasional diparameterisasi pada adapter; timestamp memakai DB time
untuk menghindari perbedaan jam antarworker. Pengukuran execution plan dilakukan
pada database disposable sebelum runtime code; hasil dicatat di bawah.

## Migration, deployment, dan peralihan driver

Pasangan baru: `20261101000000_create_job_outbox_table.sql`. Tidak mengubah
migration/tabel lama, nilai ID lama, JWT subject, atau kontrak response user.
DDL hanya membuat tabel baru; tidak melakukan backfill atau lock seluruh tabel
users/jobs. Readiness repository membandingkan migration secara ketat: lakukan
maintenance deployment terkoordinasi (migrate, kemudian binary baru), atau
rancang kebijakan kompatibilitas terpisah sebelum rolling deploy.

Pilihan driver berlaku untuk job baru. Job lama di `jobs` tetap milik driver
database; catatan di `job_outbox` tetap milik driver Redis. Mengganti env tidak
memindahkan atau menghapus backlog. Operator menjalankan worker lama/override
database sampai pending/running lama habis, dan tetap mempertahankan worker
Redis+relay sampai outbox aktif habis jika kembali ke database.

Redis worker memiliki relay terkelola secara default untuk `make serve`;
deployment dapat menjalankan binary relay khusus. Kedua cara menggunakan lease
SQL yang sama sehingga beberapa relay tidak membuat dua pemilik aktif.

## Bukti verifikasi desain

Kandidat migration diterapkan pada database sementara terisolasi sebelum runtime
code. Database fixture dihapus sesudah pengukuran. Server lokal: PostgreSQL
16.15 dan MySQL 8.0.46. Dataset 50.000 UUIDv7: 40.000 completed, 9.000 delayed
pending, 500 ready pending, 500 expired running. Statistik diperbarui sebelum
pengukuran; cache bukan bagian dari jalur SQL ini.

| Kandidat | PostgreSQL: actual plan / execution | MySQL: actual plan / execution |
|---|---|---|
| Relay ready LIMIT 1 | `job_outbox_relay_idx`, Index Cond `relay_due_at <= statement_timestamp()`, 1 row, 0,119 ms | Range `job_outbox_relay_idx`, 1 row, 0,272 ms |
| Relay tanpa due, seluruh aktif delayed | Index Cond yang sama, 0 row, 0,276 ms | Range yang sama, 0 row, 0,067 ms |
| Retention LIMIT 1000 | `job_outbox_retention_idx`, status + timestamp Index Cond, 1000 rows, 1,197 ms | Covering range retention index, 1000 rows, 0,676 ms |
| Lookup execution PK | `job_outbox_pkey`, 1 row, 0,090 ms | Constant primary-key lookup, 1 row |

Plan relay/retensi tidak memerlukan filesort atau pemindaian semua 50.000 row.
Kasus PostgreSQL empty-due diuji segera setelah mengubah 1.000 row yang due;
951 buffer hits mencakup versi indeks lama yang belum divacuum. Ini menunjukkan
alasan pemeliharaan MVCC tetap penting, bukan indikasi future backlog dibaca
seluruhnya. Angka latency adalah sampel lokal satu eksekusi; tidak diklaim
sebagai percentile, kapasitas produksi, atau bukti MySQL 8.4 sudah diuji.

Perintah pengukuran: PostgreSQL `EXPLAIN (ANALYZE, BUFFERS)` dan MySQL
`EXPLAIN ANALYZE`, masing-masing untuk SQL pada bagian pola query, lookup PK,
dan seleksi retensi berurutan. Statement dengan locking dijalankan hanya pada
fixture disposable. Bentuk query runtime harus tetap memakai predicate/sort ini.

## Verifikasi perilaku yang diperlukan

- Transaksi rollback tidak meninggalkan outbox; enqueue dedupe tetap satu record.
- Semua creation path outbox menghasilkan UUIDv7; lease token berbeda dari PK.
- Relay crash sebelum publish dan setelah publish tetap memulihkan delivery.
- Token relay lama tidak dapat mengubah record setelah claim/reclaim eksekusi.
- Concurrent Redis delivery hanya mendapatkan satu lease eksekusi yang valid.
- Claim PK menunggu lock publish singkat lalu mengeksekusi tanpa menunggu relay
  berikutnya; timeout klaim tidak menahan satu-satunya slot pool SQL.
- ACK/fail dengan token kedaluwarsa atau pemilik lama ditolak.
- Backoff, permanent failure, dan crash pada percobaan terakhir menjadi terminal.
- Completed tidak dieksekusi ulang; retention dibatasi 1000 dan tidak menghapus failed.
- Kehilangan Redis/publish error tidak menghilangkan catatan SQL.
- Driver matrix, transaksi user+job, serta shutdown/recovery diuji pada database
  sebenarnya dan Redis disposable sebelum commit.
