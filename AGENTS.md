# Instruksi agent — Rust API Skeleton

Aturan **WAJIB/DILARANG** berikut merupakan kriteria penerimaan semua pekerjaan.
Baca [standar desain lengkap](docs/system-design-rules.md) sebelum merancang
fitur, query, model, repository, job, atau migration. Contoh lama dan output
generator tidak mengalahkan aturan ini.

## 1. Clean code

- Pertahankan alur **route → handler → service → repository trait → adapter**.
  Handler mengurus HTTP/JSON; service memiliki use case/policy; SQL dan detail
  PostgreSQL/MySQL berada di adapter. Gunakan dependency injection melalui
  constructor/AppState, bukan dependency/koneksi tersembunyi per request.
- Modul/fungsi harus memiliki tanggung jawab jelas, nama domain yang konsisten,
  tipe eksplisit, dan duplikasi minimal. Hindari abstraksi spekulatif serta fungsi
  serba guna yang mencampur HTTP, SQL, policy, dan serialisasi.
- Tangani error secara eksplisit. Jangan menyembunyikan kegagalan dengan default
  sukses, blanket lint suppression, atau `unwrap`/`expect` pada input, konfigurasi,
  dan I/O produksi. Pisahkan entity internal dari request/response/read DTO.

## 2. Desain database dan migration sebelum fitur

- **Sebelum kode repository/service/handler/job fitur**, tulis
  `docs/design/<nama_fitur>.md`: tabel/relasi, PK/FK, constraint, proyeksi kolom,
  scope akses, filter/sort/cursor, batas page/batch, perkiraan volume, query budget,
  indeks, rencana migration, dan cara verifikasinya.
- Rancang pola baca/tulis terlebih dahulu. Buat pasangan migration PostgreSQL dan
  MySQL beserta indeks/constraint yang diperlukan **sebelum implementasi fitur**.
  Dilarang menunda indeks dengan alasan akan dioptimalkan setelah fitur selesai.
- Jika indeks/skema yang ada cukup, dokumentasikan buktinya; jangan membuat
  migration atau indeks duplikat hanya untuk memenuhi checklist.
- Terapkan kandidat migration dan ukur kandidat query pada database disposable
  dengan data representatif. Verifikasi plan pada kedua driver sebelum fitur
  dinyatakan selesai; data kecil saja tidak membuktikan performa pada volume besar.
- Migration yang sudah diterapkan immutable. Gunakan versi baru, tinjau locking,
  backfill, rollout aplikasi, dan readiness; jangan memalsukan checksum SQLx.

## 3. Primary key wajib bernama sesuai tabel dan UUIDv7

- Setiap tabel milik aplikasi wajib mempunyai PK **`<entitas_tunggal>_id`**:
  `users.user_id`, `jobs.job_id`, `customers.customer_id`, `invoices.invoice_id`,
  `invoice_items.invoice_item_id`. Dilarang PK generik `id`, `uuid`, atau `uid`.
- Nama entitas berasal dari tabel dan harus eksplisit, termasuk mapping nama
  tidak beraturan. Jangan membentuk singular hanya dengan membuang huruf `s`.
- **Setiap PK baris baru wajib UUIDv7**, termasuk job/outbox, seed, import, dan
  tabel penghubung. Gunakan `Uuid::now_v7()` atau factory aplikasi yang setara.
  Dilarang PK auto-increment, UUIDv4/v1/nil, atau default UUID DB versi lain.
  Test creation path harus memeriksa versi 7, bukan sekadar UUID dapat di-parse.
- PostgreSQL memakai `UUID`. Representasi MySQL saat ini adalah
  `CHAR(36) CHARACTER SET ascii COLLATE ascii_bin`; tipe/kolasi PK dan FK harus
  konsisten. Perubahan representasi memerlukan desain dan migration tersendiri.
- FK menyebut entitas tujuan, misalnya `invoice_items.invoice_id →
  invoices.invoice_id`. Nama peran seperti `created_by_user_id` harus memiliki
  target jelas. Definisikan constraint dan indeks pendukung akses FK.
- Tabel penghubung tetap memiliki PK UUIDv7 sendiri; gunakan `UNIQUE` terpisah
  untuk kombinasi relasi yang harus unik. Business key bukan pengganti PK ini.
- Scope aturan adalah tabel milik aplikasi; metadata SQLx/sistem eksternal tetap
  mengikuti kontraknya. UUIDv7 bukan secret. Nonce, lease/fencing token, dan
  credential mengikuti kebutuhan keamanan masing-masing, bukan aturan PK.

## 4. Query eksplisit, minimal, dan tanpa N+1

- **WAJIB menuliskan nama kolom; DILARANG `SELECT *`, `alias.*`, dan `RETURNING *`**,
  termasuk di subquery, CTE, atau query builder. Agregat/ekspresi harus eksplisit
  dengan alias output yang jelas. Gunakan alias tabel/kolom yang tidak ambigu.
- Ambil hanya kolom yang dibutuhkan use case. List/profil publik tidak boleh
  mengambil `password_hash`, token, secret, atau JSON/BLOB yang tidak dipakai.
  Pisahkan proyeksi autentikasi dari proyeksi list/detail.
- Bind semua nilai SQL. Identifier, operator, dan sort dinamis hanya boleh
  berasal dari allowlist kode; jangan interpolasi input pengguna menjadi SQL.
- **Dilarang N+1**, termasuk query per item di loop, async map, serializer, atau
  callback. Menjalankannya paralel atau menutupinya dengan cache bukan perbaikan.
- Gunakan JOIN yang menjaga kardinalitas, agregasi, atau batch fetch relasi.
  Query count mengikuti anggaran tetap per jenis relasi atau rumus chunk
  terbatas yang didokumentasikan, bukan satu query tambahan per entitas.
- Batasi ukuran `IN`, batch, jumlah child per parent, dan response. Jangan
  mengganti N+1 dengan pemuatan seluruh tabel/relasi tanpa batas.
- Uji jumlah query untuk beberapa ukuran halaman termasuk maksimum, dengan
  cache kosong. Scope tenant/policy tetap wajib pada query relasi.

## 5. Pagination wajib cursor

- Semua list yang dipaginasi memakai **keyset/cursor pagination**. Dilarang
  `OFFSET`, `LIMIT offset,count`, atau `page=N` yang dihitung menjadi offset.
  Job/export yang membaca banyak data juga memakai batch dengan cursor.
- Ikuti batas API saat ini: default `limit=20`, rentang `1..100`. Batas batch
  background harus eksplisit dan dijustifikasi sesuai beban operasi.
- Gunakan urutan deterministik dengan PK unik sebagai tie-breaker, misalnya
  `created_at DESC, customer_id DESC`. Predicate harus cocok dengan seluruh
  kolom/arah sort. Pilih sort key immutable atau definisikan konsistensinya.
- Ambil `limit + 1`, buang lookahead, dan bentuk `next_cursor` dari item terakhir
  yang dikirim. Jangan menambahkan total-count otomatis pada setiap page tanpa
  kebutuhan dan rancangan performa tersendiri.
- First/next page wajib memakai filter, tenant/policy scope, dan soft-delete
  predicate yang sama. Scope akses berasal dari identitas tepercaya di server.
- Cursor fitur baru/perubahan kontrak harus berversi dan terikat pada resource,
  sort, serta filter/scope relevan. Batasi decoding, validasi tipe/presisi, dan
  tolak konteks yang tidak cocok. Base64 bukan enkripsi, integritas, atau izin.
- Jangan menerapkan limit pada JOIN one-to-many yang menggandakan parent.
  Paginasi parent terlebih dahulu lalu batch relasi, atau buktikan hasil query
  tepat satu baris per parent sebelum limit diterapkan.

## 6. Optimasi dan indexing ketat

- Setiap indeks harus memiliki query/constraint sebagai alasan. Rancang indeks
  komposit sesuai equality filter/scope, join, range, dan sort aktual; periksa
  urutan kolom serta prefix yang dapat digunakan.
- Tinjau query dengan/tanpa filter opsional, first/next page, lookup FK,
  uniqueness, dan akses worker/retention yang relevan.
- Hindari indeks duplikat, indeks setiap kolom tanpa alasan, dan indeks terlalu
  lebar. Nilai biaya write, ukuran indeks, dan fan-out. Cache tidak menggantikan
  desain query/index yang benar.
- Periksa `EXPLAIN`; ukur dengan `EXPLAIN ANALYZE` pada data disposable karena
  statement benar-benar dijalankan. Simpan plan, skala data, latency, query count,
  dan kesimpulan dalam dokumen desain fitur untuk PostgreSQL dan MySQL.
- Perbaiki scan besar yang tidak perlu, sort/materialisasi mahal, dan estimasi
  baris yang meleset. Jangan memaksa indeks demi tampilan plan: sequential scan
  pada tabel kecil dapat wajar. Justifikasi harus memakai data representatif.
- Jangan mengasumsikan fitur indeks/planner identik pada kedua database.
  Verifikasi semantik hasil dan optimasi pada masing-masing driver.

## 7. Legacy, generator, dan verifikasi

- Baca [riwayat dan status penyesuaian](docs/system-design-rules.md#9-status-kode-saat-aturan-ditambahkan)
  serta [desain contoh saat ini](docs/design/example-rule-alignment.md). Contoh
  user/queue dan generator memakai PK bernama, pembuatan UUIDv7, proyeksi minimal,
  dan cursor berkonteks. PK generik dalam migration historis hanya menggambarkan
  skema sebelum migration rename; jangan menyalinnya ke fitur baru.
- Jangan diam-diam mengganti kolom, identifier tersimpan, kontrak API/cursor,
  atau payload job. Rancang migration baru dan rollout kompatibilitas terlebih
  dahulu; rename kolom tidak boleh meregenerasi ID. Laporkan ketidaksesuaian yang
  masih ada tanpa mengklaim semua legacy telah mematuhi aturan baru.
- Tinjau output `make model`, `make repository`, dan `make migration` sebelum
  dipakai. Berhasil compile belum membuktikan PK, UUIDv7, proyeksi, indeks, atau
  policy sudah benar. Jalankan `make test-scaffolds` jika generator berubah.
- Tambahkan test perilaku yang relevan untuk query count, cursor, versi PK,
  constraint, transaksi, dan ekuivalensi driver. Ignored/belum dijalankan bukan
  bukti lulus. Ringkasan harus menyebut keputusan desain, verifikasi, batas yang
  tersisa, dan dampak kompatibilitas.
- CI GitHub adalah informasi tambahan; kegagalan akibat kuota tidak menggantikan
  hasil verifikasi lokal. Pertahankan graceful shutdown dan cleanup proses anak;
  SIGTERM berulang tidak memperpanjang deadline, SIGINT saat drain memaksa cleanup.

## 8. Invariant keamanan dan runtime repository

- Keep this project API-only. Handlers extract/authorize inputs and delegate to
  services; services depend on repository traits rather than SQLx pools.
- Support PostgreSQL and MySQL for every persistence change. Keep driver-specific
  SQL in adapters. Verify default, postgres-only, and mysql-only builds.
- All SQL values are bound parameters. Never interpolate user-controlled IDs,
  search strings, sort expressions, or table names into SQL.
- Store timestamps in UTC and return UTC RFC3339 timestamps. MySQL DATETIME
  conversion and binary-collation text decoding belong in the adapter.
- Normalize DTO text before validation. Do not normalize passwords. Reject
  privileged/unknown fields instead of mass assignment.
- Do not serialize/debug/log passwords, password hashes, JWTs, DSNs, SMTP
  credentials, or raw request payloads. Validation errors must omit rejected values.
- New users are role `user`. Authorization reads the current database record.
  Add explicit policy checks for new endpoints; authentication alone is not authorization.
- Cache only reproducible data through the cache facade. Include environment and
  tenant scope in namespaces/keys, use bounded TTLs, and define invalidation behavior.
  Optional cache failure must not change authorization or transactional queue semantics.
- Preserve the user+welcome-job transaction. Domain writes and associated durable
  jobs must commit atomically when the business operation requires both.
- Queue handlers must tolerate at-least-once execution. ACK/retry must preserve
  claim-token and unexpired-lease guards. Use safe error categories in job records.
- Keep CPU-heavy password operations in bounded blocking tasks, with permits
  retained until the actual task completes, even when an HTTP waiter is canceled.
- Add new migration versions to both database directories. Do not edit applied
  migrations. Update readiness compatibility policy deliberately when changing rollout behavior.
- Prefer small modules and explicit types; use `AppResult` for application paths
  and `anyhow::Result` for bootstrap/process boundaries. No unsafe code.
- Run `make check` before proposing changes. Run both real-database suites when
  changing SQL, migrations, auth/repository integration, or queue behavior. Report
  unrun checks honestly; ignored tests are not evidence of integration success.
- Run `make test-redis` on a disposable Redis instance for cache-driver changes.
  Preserve timeout, payload-size and secret-sanitization boundaries; never flush
  unrelated Redis keys in tests or application cache helpers.
- Never commit `.env`, generated credentials, build outputs, or local test data.
  Preserve `Cargo.lock` and the pinned toolchain for reproducible builds.
