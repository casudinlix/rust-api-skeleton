# Standar desain sistem dan akses data

Dokumen ini menjabarkan aturan wajib di [AGENTS.md](../AGENTS.md) untuk agent dan
developer Rust API Skeleton: clean code, query tanpa N+1, proyeksi eksplisit,
cursor pagination, PK bernama sesuai tabel dengan UUIDv7, serta desain indeks
dan migration sebelum implementasi fitur.

Scope aturan adalah skema milik aplikasi. Metadata seperti `_sqlx_migrations`
mengikuti kontrak SQLx. Bagian 9 mencatat baseline saat aturan ditambahkan dan
penyesuaian contoh existing yang sudah dilakukan, termasuk batas data historis.

## 1. Alur kerja sebelum membuat fitur

Urutan wajib untuk fitur yang mengakses atau mengubah database:

1. Baca aturan, model, kontrak API, migration, dan query terkait yang sudah ada.
2. Tulis `docs/design/<nama_fitur>.md` memakai template bagian 8. Tentukan scope
   akses, kebutuhan data, volume, pola baca/tulis, dan batas operasi.
3. Rancang PK/FK, constraint, query beserta kolomnya, cursor, anggaran jumlah
   query, dan indeks untuk setiap pola akses.
4. Tulis pasangan migration PostgreSQL/MySQL beserta indeks/constraint yang
   diperlukan. Bila skema/indeks yang ada cukup, dokumentasikan buktinya.
5. Terapkan migration pada database disposable dan periksa kandidat query
   memakai data representatif. Perbaiki desain sebelum implementasi repository,
   service, handler, atau job fitur tersebut.
6. Implementasikan query yang telah ditinjau. Jika bentuk query berubah, ulangi
   pemeriksaan plan, kemudian uji perilaku dan kedua adapter database.
7. Laporkan bukti verifikasi dan rencana rollout. Fitur belum selesai bila
   migration, indexing, scope, atau batas query masih ditunda.

Yang wajib adalah analisis akses data dan kecukupan indeks, bukan penambahan
indeks tanpa tujuan. Bukti indeks existing sudah cukup menghindari redundansi.

## 2. Clean code dan pembagian tanggung jawab

| Lapisan | Tanggung jawab |
|---|---|
| Route/middleware | Endpoint, guard, batas request, dan policy lintas endpoint |
| Handler | Ekstraksi/validasi input, pemanggilan service, response JSON |
| Service | Use case, business rule, otorisasi domain, koordinasi transaksi/dependency |
| Repository trait | Kontrak data dengan tipe domain/read DTO yang jelas |
| Adapter database | SQL terikat parameter, row mapping, transaksi dan detail driver |
| Job/scheduler | Dispatch terjadwal dan eksekusi idempotent melalui service/repository |

Fungsi memiliki tujuan jelas. Pisahkan autentikasi, list, detail, dan write
ketika kebutuhan kolom/policy berbeda. Gunakan read DTO untuk list alih-alih
memuat entity penuh yang membawa credential.

Hindari SQL di handler, koneksi per request, I/O tersembunyi dalam serializer,
duplikasi SQL di service, blocking CPU pada runtime async, dan error yang berubah
menjadi sukses palsu. Gunakan tipe/error eksplisit dan dependency injection.
Panjang fungsi bukan satu-satunya ukuran kualitas; kemudahan review, batas
tanggung jawab, serta invariant yang diuji harus menjadi pertimbangan.

## 3. Konvensi PK, UUIDv7, dan relasi

### Nama kolom

Tabel menggunakan `snake_case`. Tentukan nama entitas tunggal secara eksplisit,
lalu bentuk PK sebagai `<entitas>_id`.

| Tabel | PK wajib |
|---|---|
| `users` | `user_id` |
| `jobs` | `job_id` |
| `customers` | `customer_id` |
| `companies` | `company_id` |
| `invoices` | `invoice_id` |
| `invoice_items` | `invoice_item_id` |
| `user_roles` | `user_role_id` |
| `people` | `person_id`, mapping tunggal dicatat eksplisit |

PK generik `id`, `uuid`, atau `uid` tidak boleh dipakai pada skema baru. Gunakan
nama yang sama pada model/read DTO baru agar relasi mudah ditelusuri. Perubahan
kontrak API lama membutuhkan strategi kompatibilitas, bukan rename otomatis.

FK mengikuti entitas tujuan, misalnya `invoice_items.invoice_id` mereferensikan
`invoices.invoice_id`. Dua peran menuju tabel sama memakai nama peran yang jelas,
misalnya `created_by_user_id` dan `approved_by_user_id`. Catat target constraint,
aturan delete/update, serta indeks pendukungnya.

Tabel penghubung memiliki PK UUIDv7 sendiri; tambahkan `UNIQUE` untuk pasangan
relasi yang harus unik. PK tidak menggantikan constraint business key/deduplikasi.

### Pembuatan UUIDv7

Semua jalur pembuatan baris baru memakai UUIDv7: API, job, outbox, seeder, import,
dan operasi batch. Contoh:

```rust
let customer_id = uuid::Uuid::now_v7();
```

Factory aplikasi harus menghasilkan versi 7. Test creation path memeriksa
`get_version_num() == 7`; parsing UUID saja tidak membuktikan versinya. Jangan
menerima PK internal baru dari payload create publik tanpa kontrak import khusus;
external reference disimpan sebagai field terpisah.

PostgreSQL menggunakan `UUID`. Representasi MySQL saat ini memakai
`CHAR(36) CHARACTER SET ascii COLLATE ascii_bin` dengan bentuk canonical
konsisten. FK harus mempunyai tipe/kolasi yang cocok. Perubahan representasi
memerlukan desain konversi, perbandingan/sort, backfill, FK, dan kompatibilitas
adapter tersendiri.

UUIDv7 membawa komponen waktu, tetapi bukan secret atau bukti urutan bisnis global
antarproses. Simpan waktu kejadian/`created_at` secara eksplisit dan gunakan
tie-breaker unik untuk pagination. Lease token dan credential tidak otomatis
menjadi UUIDv7 karena PK menggunakannya. Dasar format/API:
[RFC 9562](https://www.rfc-editor.org/rfc/rfc9562.html#section-5.7) dan
[dokumentasi uuid](https://docs.rs/uuid/latest/uuid/struct.Uuid.html#method.now_v7).

## 4. Proyeksi query dan pencegahan N+1

### Kolom eksplisit dan minimum

Larangan `SELECT *`, `alias.*`, dan `RETURNING *` berlaku pada query utama,
subquery, CTE, dan query builder. Tuliskan kolom serta alias yang diperlukan.
Contoh proyeksi list:

```sql
SELECT c.customer_id, c.name, c.created_at
FROM customers AS c
WHERE c.company_id = $1
ORDER BY c.created_at DESC, c.customer_id DESC
LIMIT $2;
```

Contoh SQL di dokumen ini adalah rancangan fitur baru dengan PK bernama, bukan
perintah untuk langsung dijalankan pada tabel legacy. Adapter memakai placeholder
masing-masing driver dan semua nilai tetap di-bind. Scope company/tenant, bila
fitur memilikinya, berasal dari identitas dan policy tepercaya di server.

Nama kolom eksplisit belum cukup bila semua kolom tetap diambil. Jangan memilih
`password_hash` pada list user atau JSON/BLOB yang tidak diperlukan. Agregat
mempunyai output jelas, misalnya `COUNT(c.customer_id) AS customer_count`.
Untuk existence check, gunakan `EXISTS` dengan proyeksi jelas; jangan menghitung
semua baris hanya untuk mengetahui apakah ada kecocokan.

### Anggaran query harus dapat dibuktikan

Contoh anggaran: satu query mengambil halaman invoice, satu batch mengambil
customer terkait, dan satu batch mengambil ringkasan item. Jumlahnya tetap tiga
untuk halaman dengan 1, 20, atau 100 invoice. Bila perlu chunking karena batas
parameter/payload, catat rumus dan ukuran chunk maksimum.

Dilarang memanggil `find_customer(invoice.customer_id)` sekali per invoice,
termasuk melalui async task paralel atau serializer. Cache hit tidak menghapus
N+1 saat cache miss. Ukur query count dengan cache kosong dan variasi relasi,
termasuk banyak parent dengan banyak child.

Batch harus membatasi input dan hasil. Hindari `IN` raksasa atau memuat seluruh
child per parent. Gunakan agregasi untuk ringkasan, batas hasil terdokumentasi,
atau endpoint cursor terpisah untuk relasi besar. Scope/policy tetap diterapkan
saat mengambil relasi.

Pada JOIN one-to-many, pilih page parent terlebih dahulu. `LIMIT 21` pada hasil
join dapat membatasi baris child alih-alih jumlah parent. Gunakan CTE/subquery
parent atau batch kedua; query yang langsung dipaginasi harus dibuktikan
menghasilkan satu baris per parent.

## 5. Cursor pagination wajib

Endpoint list menerima `cursor` opsional dan `limit`; default 20, minimum 1,
maksimum 100. Dilarang offset/nomor page. Export/background scan juga memakai
batch terbatas dengan cursor.

Default sort yang sesuai adalah `created_at DESC, customer_id DESC`. PK menjadi
tie-breaker ketika timestamp sama. Gunakan sort key immutable; sort pada kolom
yang dapat berubah membutuhkan definisi konsistensi eksplisit.

Contoh page berikutnya, PostgreSQL:

```sql
SELECT c.customer_id, c.name, c.created_at
FROM customers AS c
WHERE c.company_id = $1
  AND (c.created_at, c.customer_id) < ($2, $3)
ORDER BY c.created_at DESC, c.customer_id DESC
LIMIT $4;
```

Padanan predicate MySQL:

```sql
SELECT c.customer_id, c.name, c.created_at
FROM customers AS c
WHERE c.company_id = ?
  AND (c.created_at < ? OR (c.created_at = ? AND c.customer_id < ?))
ORDER BY c.created_at DESC, c.customer_id DESC
LIMIT ?;
```

Timestamp cursor di-bind dua kali pada contoh MySQL. Nilai `LIMIT` adalah
`limit + 1` setelah validasi. Buang lookahead dan buat `next_cursor` dari item
terakhir yang dikirim. First-page memakai query tanpa predicate cursor dengan
scope/filter/sort sama. Untuk ascending, sesuaikan operator dan arah sort.
Kandidat query tetap harus diverifikasi dengan execution plan nyata.

Kontrak cursor baru memuat versi, resource, sort/direction, nilai sort terakhir,
dan pengikat konteks filter/scope relevan. Pembanding konteks dibentuk dari request
yang dinormalisasi serta identitas terotorisasi di server. Tolak ketidakcocokan;
jangan mengambil tenant atau izin dari cursor.

Batasi encoded/decoded payload, tolak field/type tidak sah, dan validasi UUID
serta timestamp sesuai presisi kedua database. Base64 hanya encoding. Jika
integritas payload diperlukan, desain autentikasi cursor secara eksplisit;
scope SQL tetap wajib walaupun cursor ditandatangani.

Test mencakup timestamp sama, page kosong/terakhir, batas limit, cursor rusak,
perubahan filter/scope, dan insert/delete di antara page. Cursor tidak otomatis
memberikan snapshot seluruh dataset. Export dengan tuntutan snapshot memerlukan
rancangan cutoff/snapshot tersendiri.

Urutan unik dan biaya offset dibahas dalam
[dokumentasi LIMIT PostgreSQL](https://www.postgresql.org/docs/16/queries-limit.html).

## 6. Indeks dan constraint dalam migration

Untuk contoh list customer per company, indeks kandidat kedua driver:

```sql
CREATE INDEX customers_company_created_pk_idx
    ON customers (company_id, created_at DESC, customer_id DESC);
```

Indeks mengikuti scope equality company lalu urutan cursor. Definisi tabel
memakai `customer_id UUID PRIMARY KEY` untuk PostgreSQL, atau
`customer_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL` beserta
`PRIMARY KEY (customer_id)` untuk MySQL. Tipe FK `company_id` harus cocok dengan
`companies.company_id` dan constraint harus sesuai domain. Aplikasi menghasilkan
UUIDv7 sebelum insert.

Contoh ini bukan resep universal. Filter status, range tanggal, atau sort berbeda
memerlukan tinjauan kombinasi/selektivitas. Query dengan dan tanpa filter opsional
bisa memerlukan strategi berbeda. Periksa prefix indeks komposit pada
[PostgreSQL](https://www.postgresql.org/docs/16/indexes-multicolumn.html) dan
[MySQL](https://dev.mysql.com/doc/refman/8.4/en/multiple-column-indexes.html).

Setiap migration fitur harus menjawab:

- PK/FK mana yang digunakan, tipe/nullability, dan aturan delete/update?
- Constraint apa yang menjaga business key, deduplikasi, dan nilai valid?
- Indeks mana yang mendukung filter, join, cursor, lookup, dan maintenance?
- Apakah sudah tercakup oleh PK/UNIQUE/indeks komposit existing?
- Apa dampaknya pada write, ukuran penyimpanan, dan locking DDL?
- Bagaimana kedua database menjaga hasil/policy setara dengan optimasi
  masing-masing?

Hindari fungsi/cast pada kolom indeks yang menyulitkan penggunaan indeks. Untuk
pencarian berbasis ekspresi, desain dukungan indeks masing-masing driver dan uji
plan-nya. Jangan mengindeks semua kolom, menumpuk indeks duplikat, atau memindahkan
masalah query ke cache.

Migration yang sudah diterapkan immutable. Rename/penambahan indeks, kolom,
PK/FK, dan constraint memakai versi baru pada kedua direktori. Data besar
memerlukan batch/cursor, tinjauan locking, dan rencana rollout.

## 7. Bukti optimasi sebelum fitur selesai

Rancangan dan kandidat query diperiksa sebelum kode fitur. Implementasi akhir
harus tetap cocok dengan rancangan yang diukur. Untuk setiap pola akses penting:

1. Siapkan data representatif: ukuran tabel, distribusi scope/filter, fan-out,
   timestamp kembar, dan selektivitas. Perbarui statistik database bila perlu.
2. Jalankan `EXPLAIN` pada kedua driver untuk first/next page, filter opsional,
   dan query relasi. Pada data disposable gunakan `EXPLAIN (ANALYZE, BUFFERS)`
   pada PostgreSQL dan `EXPLAIN ANALYZE` pada MySQL.
3. Catat versi DB, skala data, query/bind yang disanitasi, indeks, estimated/actual
   rows, loops, sort/materialisasi, latency, query count, serta informasi buffer
   atau rows examined bila tersedia.
4. Perbaiki scan/sort besar yang tidak perlu, fan-out tak terbatas, dan estimasi
   keliru. Bandingkan sebelum/sesudah perubahan relevan dan tulis alasan menerima
   plan akhir terhadap target beban fitur.

`EXPLAIN ANALYZE` benar-benar menjalankan statement. Gunakan data disposable dan
operasi terkontrol; jangan menguji write produksi secara tidak sengaja. Referensi:
[PostgreSQL EXPLAIN](https://www.postgresql.org/docs/16/sql-explain.html) dan
[MySQL EXPLAIN](https://dev.mysql.com/doc/refman/8.4/en/explain.html).

Tidak semua sequential scan buruk dan tidak semua index scan bagus. Optimizer
dapat memilih scan pada tabel kecil atau predicate yang mencakup banyak baris.
Jangan force-index agar checklist tampak lulus; buktikan biaya dan hasil pada
skala target. Bila lingkungan pengukuran belum tersedia, laporkan verifikasi
performa sebagai belum selesai.

## 8. Template dokumen desain fitur

Simpan di `docs/design/<nama_fitur>.md` dan lengkapi sebelum kode fitur:

```markdown
# Desain data: <nama fitur>

## Use case dan skala

- Operasi API/job, volume sekarang/target, read/write rate, fan-out:
- Scope tenant/policy dan sumber identitas tepercaya:
- Target latency, batas response/page/batch:

## Skema dan integritas

- Tabel → entitas tunggal → PK bernama, tipe, factory UUIDv7:
- Kolom, FK/aksi referensial, UNIQUE/CHECK, business key:
- Dampak pada API/DTO/cursor/payload job dan data lama:

## Pola query

| Operasi | Kolom hasil | Filter/scope | Join | Sort/cursor | Query budget | Indeks |
|---|---|---|---|---|---|---|
| ... | ... | ... | ... | ... | ... | ... |

- SQL/bind kedua driver, termasuk first/next page:
- Batch relasi, chunk maksimum, batas child, concurrent writes:

## Migration dan rollout

- Pasangan versi migration; alasan indeks baru/existing sudah cukup:
- Locking, backfill dengan cursor, kompatibilitas deploy, readiness:

## Verifikasi

- Dataset/versi DB, EXPLAIN kedua driver, dan kesimpulannya:
- Query count page kecil/maksimum dengan cache kosong:
- Test cursor, UUIDv7 PK, scope akses, constraint, transaksi:
- Perintah yang dijalankan, hasil, dan batas yang belum diverifikasi:
```

Jalankan `make check`, kedua suite database untuk perubahan persistence, dan
`make test-scaffolds` untuk perubahan generator. Perubahan cache driver juga
memerlukan `make test-redis`. Test ignored/belum dijalankan bukan bukti lulus.
CI GitHub bersifat tambahan; kuota runner bukan pengganti hasil lokal.

## 9. Status kode saat aturan ditambahkan

Inventaris historis berdasarkan baseline commit `7ddd63a`, dengan status setelah
migration `20261102000000_align_example_primary_keys.sql`:

| Area | Kondisi baseline | Implementasi saat ini |
|---|---|---|
| PK schema | Kedua driver memakai `users.id` dan `jobs.id` | Migration baru mempertahankan semua nilai dan menamai kolom `user_id`/`job_id`; model, adapter, DTO, route parameter, dan SQL diagnosis memakai nama tersebut |
| UUID user baru | `src/services/auth_service.rs` memakai `Uuid::now_v7()` | Service tetap membuat v7; kedua repository menolak PK baru selain versi 7/variant RFC sebelum membuka transaksi |
| UUID job baru | Queue dan welcome-job insert memakai `Uuid::new_v4()` | Queue, welcome-job, dan outbox membuat UUIDv7; lease token tetap terpisah |
| Lease token | Queue memakai token UUIDv4 terpisah | Bukan PK; tetap mengikuti aturan fencing/keamanan |
| Proyeksi list user | Kolom eksplisit tetapi `password_hash` masih diambil | `User` adalah proyeksi publik; `UserCredentials` hanya untuk login; list/detail/authenticate/write-result tidak mengambil hash |
| Pagination user | Keyset `(created_at, id)`, lookahead, limit maksimum 100 | Keyset `(created_at, user_id)` dengan indeks, lookahead, batas 100, dan cursor versi 1 terikat resource/sort/scope |
| Generator | Model/parameter repository memakai `id` | NAME entity singular eksplisit menghasilkan `<entity>_id`, factory `new()`/`Default` UUIDv7, dan parameter repository yang konsisten |
| Queue claim/retention | Predicate pending/running terpisah dan delete completed tidak dibatasi | Indeks generated `claimable_at` untuk claim; retention memilih maksimal 1.000 PK terurut dan menghapusnya dalam transaksi |

Lihat [desain dan bukti penyesuaian](design/example-rule-alignment.md) serta
[catatan verifikasi](verification.md) untuk kandidat query pada 50.000 baris per
tabel/driver, query budget, pengujian migration/kontrak, dan langkah rollout.
File migration awal tetap berisi `id` karena checksum history immutable; skema
hasil **seluruh** migration memakai nama PK yang diwajibkan. Fixture upgrade
sengaja memakai UUIDv4 dan kolom lama untuk membuktikan preservasi data, bukan
sebagai contoh creation path baru.

Penyesuaian legacy memakai migration baru dan rollout terencana. Rename kolom
tidak boleh meregenerasi identifier baris. Jangan mengganti UUID lama diam-diam
agar tampak v7: FK, JWT subject, payload antrean, deduplication key, dan referensi
eksternal dapat bergantung pada nilai tersebut. ID lama tetap harus dapat dibaca
selama transisi. Jika penggantian nilai ID memang diperlukan, rancang pemetaan
dan migrasi referensi tersendiri. Laporkan kepatuhan baris baru dan keterbatasan
data lama secara terpisah.
