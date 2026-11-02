# Database migration, rollback, fresh, dan seeding

Skeleton memakai SQLx migration **forward** yang tetap immutable di
`migrations/{postgres,mysql}`. Rollback disediakan melalui companion SQL terpisah
di `rollbacks/{postgres,mysql}` agar file migration historis dan checksum SQLx
tidak perlu diubah.

## Command

```bash
make migrate

make migrate-rollback
make migrate-rollback STEP=2

make migrate-fresh
make migrate-fresh SEED=1
make migrate-fresh CLASS=CustomerSeeder

make db-seed
make db-seed CLASS=CustomerSeeder

make seeder NAME=CustomerSeeder
```

Artisan equivalent:

```bash
cargo run --locked --bin artisan -- migrate:rollback --step 1
cargo run --locked --bin artisan -- migrate:fresh --seed
cargo run --locked --bin artisan -- migrate:fresh --seeder CustomerSeeder
cargo run --locked --bin artisan -- db:seed
cargo run --locked --bin artisan -- db:seed --class CustomerSeeder
cargo run --locked --bin artisan -- make:seeder CustomerSeeder
```

## Rollback companion

`make migration NAME=create_customers_table` membuat empat file:

```text
migrations/postgres/<version>_create_customers_table.sql
rollbacks/postgres/<version>_create_customers_table.sql
migrations/mysql/<version>_create_customers_table.sql
rollbacks/mysql/<version>_create_customers_table.sql
```

Forward migration tetap format SQLx simple migration agar history lama tidak
diubah. Saat rollback/fresh, Artisan membangun reversible migration set sementara
dari forward SQL + rollback companion dengan versi/checksum forward yang sama,
kemudian memakai SQLx migration locking/ledger untuk menjalankan down SQL.

Rollback scaffold yang masih berisi `SCAFFOLD ONLY` ditolak. Jangan pernah
menganggap menghapus row `_sqlx_migrations` sebagai rollback schema.

`migrate:rollback` hanya dimulai dari schema yang `/ready`-compatible dan
rollback step dihitung dari migration set saat ini. Ini mencegah database partial
atau versi lama mundur ke target yang tidak dimaksudkan.

`migrate:fresh` menjalankan semua rollback companion yang sedang applied lalu
menerapkan seluruh forward migrations lagi. Ia hanya menyentuh migration yang
dikenal skeleton; tabel asing tidak dihapus secara membabi buta.

## Destructive-operation guard

Pada `APP_ENV=production`:

```bash
make migrate-rollback FORCE=1
make migrate-fresh FORCE=1
make db-seed FORCE=1
```

dibutuhkan secara eksplisit.

Rollback/fresh dapat menghapus data. Backup dan maintenance window tetap wajib
untuk production. `FORCE=1` hanya mengakui niat operator, bukan menciptakan mesin
waktu untuk data yang sudah di-DROP.

MySQL DDL melakukan implicit commit. Rollback multi-statement MySQL dapat berhenti
setelah sebagian statement berhasil. Uji rollback pada versi MySQL target dan
siapkan recovery plan sebelum production.

## Seeder

Default `DatabaseSeeder` sengaja no-op. Skeleton generik tidak membuat user,
admin, password, token, tenant, atau data bisnis contoh.

Buat seeder:

```bash
make seeder NAME=CustomerSeeder
```

Generator membuat:

```text
src/seeders/customer_seeder.rs
```

dan mendaftarkan class secara eksplisit di `src/seeders/mod.rs`.

Panggil:

```bash
make db-seed CLASS=CustomerSeeder
```

Seeder harus:

- deterministic dan idempotent bila realistis;
- memakai parameterized SQL atau boundary repository/service yang sesuai;
- memiliki volume/batch yang bounded;
- tidak mengandung production credential, token, private key, atau data personal;
- tidak dipakai untuk import besar yang lebih tepat menjadi job/tool terpisah.

`DatabaseSeeder` dapat memanggil seeder project yang memang ingin dijalankan
sebagai default.

## Verification

`make test-postgres` dan `make test-mysql` menyertakan regression
`database_maintenance` yang:

1. membuat database disposable;
2. menjalankan `migrate:fresh`;
3. memastikan schema ready;
4. rollback migration terakhir;
5. memastikan readiness turun;
6. menjalankan migration maju lagi;
7. memastikan readiness kembali;
8. menjalankan `DatabaseSeeder`;
9. menghapus database test.

Jangan menjalankan matrix ini terhadap database production.
