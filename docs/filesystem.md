# Filesystem, public storage, dan log files

Skeleton menyediakan filesystem lokal yang sengaja sederhana dan aman sebagai
baseline. Ini bukan abstraction cloud storage universal. Project turunan dapat
menambahkan S3/MinIO/object storage ketika memang dibutuhkan.

## Layout

```text
storage/
├── app/
│   ├── private/
│   └── public/
└── logs/

public/
├── assets/
└── storage -> <project>/storage/app/public
```

- `storage/app/private`: file runtime yang tidak boleh diekspos web server.
- `storage/app/public`: file runtime yang memang boleh diakses publik.
- `storage/logs`: file log bila `LOG_OUTPUT=file|both`.
- `public/assets`: asset statik yang ikut source/image.
- `public/storage`: symlink yang dibuat melalui `artisan storage:link`.

Isi runtime pada `storage/` dan symlink `public/storage` tidak di-commit.

## Konfigurasi local storage

```dotenv
FILESYSTEM_ROOT=storage/app
FILESYSTEM_MAX_FILE_BYTES=104857600
```

`FILESYSTEM_ROOT` memiliki dua disk tetap: `private` dan `public`.
`FILESYSTEM_MAX_FILE_BYTES` membatasi satu object saat read/write helper.

`AppState` menyediakan:

```rust
state.storage
```

Contoh write private:

```rust
use crate::filesystem::StorageDisk;

state
    .storage
    .put(
        StorageDisk::Private,
        "exports/report.csv",
        report.as_bytes(),
    )
    .await?;
```

Contoh write public:

```rust
state
    .storage
    .put(
        StorageDisk::Public,
        "avatars/user-123.webp",
        &image_bytes,
    )
    .await?;
```

Path dari caller wajib relatif. Helper menolak absolute path, `..`, empty path,
symlink component, object di atas size limit, dan destination directory. Write
memakai temporary file pada directory yang sama lalu rename agar file final tidak
terlihat setengah tertulis.

Jangan menyimpan upload dengan nama file mentah dari client. Bentuk nama object
sendiri dari UUIDv7/business-safe identifier dan extension allowlist.

## storage:link

Setelah checkout/source init:

```bash
make storage-link
# atau:
cargo run --locked --bin artisan -- storage:link
```

Command membuat:

```text
public/storage -> FILESYSTEM_ROOT/public
```

Command idempotent. Bila link sudah benar, tidak melakukan apa-apa. Bila sebuah
symlink menunjuk target lain:

```bash
make storage-link FORCE=1
```

`--force` hanya boleh mengganti **symlink**. File atau directory biasa tidak
pernah dihapus oleh command tersebut.

Target/link custom tersedia dari CLI:

```bash
cargo run --locked --bin artisan -- storage:link \
  --target /data/app-public \
  --link public/storage
```

Command ini saat ini Unix-only; gunakan WSL pada Windows.

## Menyajikan file public

Rust API tidak melayani static files secara default. Untuk deployment yang
filesystem-nya sama, Nginx dapat melayani public storage secara langsung.

Contoh:

```nginx
location /storage/ {
    alias /srv/my-app/storage/app/public/;
    try_files $uri =404;
    add_header Cache-Control "public, max-age=3600";
}
```

Jangan alias `storage/app/private`.

Untuk deployment multi-host/multi-region, local disk bukan shared storage.
Gunakan shared filesystem atau tambahkan adapter object-storage di project nyata;
jangan menganggap volume lokal API replica otomatis sinkron.

## File logging

Default tetap stdout karena itu paling cocok untuk container:

```dotenv
LOG_FORMAT=json
LOG_OUTPUT=stdout
RUST_LOG=info,sqlx=warn,tower_http=info
```

Untuk file:

```dotenv
LOG_OUTPUT=file
LOG_FILE_DIR=storage/logs
LOG_FILE_PREFIX=app
LOG_ROTATION=daily
LOG_MAX_FILES=14
LOG_CHANNEL_CAPACITY=8192
```

Atau stdout + file:

```dotenv
LOG_OUTPUT=both
```

Rotation yang didukung:

- `never`
- `hourly`
- `daily`

Nama file dipisahkan per process supaya beberapa binary tidak menulis file yang
sama:

```text
app-api.2026-11-02.log
app-worker.2026-11-02.log
app-scheduler.2026-11-02.log
app-relay.2026-11-02.log
app-migrate.2026-11-02.log
```

Retention berlaku per prefix/process dan dibatasi oleh `LOG_MAX_FILES`.
Writer file memakai bounded channel dan dedicated thread agar request/worker tidak
melakukan file write synchronously. Bila channel penuh, event file dapat dibuang;
stdout tidak dipengaruhi. Pada shutdown normal, guard meminta writer flush sebelum
process berakhir.

## Custom log

Gunakan structured `tracing` target, bukan membuat file log manual dari module:

```rust
tracing::info!(
    target: "app::billing",
    event = "invoice_generated",
    invoice_id = %invoice_id,
    "invoice generated"
);
```

Target dapat difilter melalui `RUST_LOG`:

```dotenv
RUST_LOG=info,app::billing=debug,sqlx=warn
```

Jangan memasukkan password, hash, JWT, DSN, secret provider, raw request body,
private key, atau payload yang tidak perlu ke log.

## Docker/Compose

Runtime image membuat `public/storage` dan Compose memasang writable named volume:

```text
app_storage:/app/storage
```

Hal ini penting karena root filesystem container tetap read-only. API, worker,
scheduler, relay, dan migration process dapat melihat storage volume yang sama
pada Compose development.

File log hanya dibuat bila `LOG_OUTPUT=file|both`; default `stdout` tetap
disarankan bila platform sudah memiliki log collector.

Jangan arahkan beberapa replica dengan role yang sama ke file log bersama pada
shared volume. Mereka akan memiliki nama file process yang sama dan rotasi file
bukan distributed lock. Untuk multi-replica gunakan `LOG_OUTPUT=stdout` dan
collector terpusat. File logging ditujukan untuk single-host/process-local
storage atau deployment yang menjamin satu writer untuk setiap nama file.
