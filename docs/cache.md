# Cache Redis dan worker

## Mengaktifkan cache

Cache tersedia melalui `AppState.cache` dan dapat diinjeksi ke service sebagai
`Arc<Cache>`. Default `CACHE_DRIVER=none` tidak membuka koneksi Redis: pembacaan
menghasilkan cache miss dan `remember` menjalankan loader.

```dotenv
CACHE_DRIVER=redis
REDIS_URL=redis://127.0.0.1:6379/0
CACHE_PREFIX=my-project:local:cache:
CACHE_DEFAULT_TTL_SECS=300
CACHE_TIMEOUT_MS=500
CACHE_MAX_VALUE_BYTES=1048576
```

Untuk Redis development, jalankan `make docker-cache`. Untuk aplikasi dalam
Compose gunakan `DOCKER_REDIS_URL=redis://redis:6379/0`. Jika Redis ada di server
eksternal, isi masing-masing URL dengan hostname/IP yang dapat dijangkau proses
tersebut. Gunakan prefix berbeda antarproyek dan environment.

| Setting | Default | Aturan |
|---|---|---|
| `CACHE_DRIVER` | `none` | `none` atau `redis` |
| `REDIS_URL` | kosong | Wajib jika driver Redis; `redis://` atau `rediss://` |
| `CACHE_PREFIX` | `rust-api-skeleton:cache:` | 1–128 byte ASCII: huruf, angka, `:`, `_`, `-`, `.` |
| `CACHE_DEFAULT_TTL_SECS` | `300` | 1–2592000 detik, maksimum 30 hari |
| `CACHE_TIMEOUT_MS` | `500` | 10–10000 ms untuk satu operasi cache |
| `CACHE_MAX_VALUE_BYTES` | `1048576` | 64–16777216 byte JSON terserialisasi |

Key suffix memiliki alfabet yang sama dengan prefix dan panjang 1–256 byte.
Prefix dan suffix digabungkan; sertakan pemisah seperti `:` di akhir prefix.
Masukkan tenant ID pada key untuk data tenant. Prefix adalah namespace, bukan
pengganti otorisasi atau isolasi akses pada aplikasi.

Koneksi dibuat secara lazy dan dikelola redis-rs dengan reconnect. `rediss://`
memverifikasi certificate server. Query parameters dan fragment URL ditolak;
driver tidak menerima opsi untuk menonaktifkan verifikasi TLS. Username/password
ACL dapat dimasukkan dalam URL dengan URL encoding yang benar.

Koneksi pertama baru disimpan setelah handshake berhasil. Kegagalan koneksi atau
autentikasi pertama tidak disimpan, sehingga panggilan berikutnya dapat mencoba
lagi. Untuk koneksi yang sudah terbentuk, reconnect jaringan didukung selama
kredensial dan handshake tetap valid. redis-rs 1.8.1 dapat mempertahankan error
handshake non-I/O saat reconnect, misalnya `AUTH`, ACL, atau `SELECT` yang ditolak.
Setelah mengubah kredensial/ACL atau memperbaiki handshake reconnect yang gagal,
perbarui konfigurasi bila perlu dan restart proses aplikasi untuk membuat
connection manager baru. `remember` tetap memakai loader selama cache gagal.

ACL terbatas perlu mengizinkan `EVAL`, `STRLEN`, `GET`, `SET`, `DEL`, dan `PING`
untuk prefix aplikasi. Koneksi redis-rs juga memakai handshake seperti `AUTH`,
`SELECT`, dan `CLIENT SETINFO` sesuai konfigurasi; uji koneksi dan reconnect
dengan ACL yang akan diterapkan.

## API penggunaan

| Operasi | Perilaku |
|---|---|
| `get::<T>(key)` | JSON menjadi `Option<T>`; key hilang menghasilkan `None` |
| `put(key, &value, ttl)` | Simpan JSON; `None` memakai TTL default |
| `forget(key)` | Hapus satu key dan laporkan apakah key ditemukan |
| `ping()` | Periksa koneksi cache; mode `none` tidak menghubungi Redis |
| `remember(key, ttl, loader)` | Gunakan cache atau jalankan loader; kegagalan cache memakai fallback |

Contoh service menerima cache dari `state.cache.clone()`:

```rust
use std::{sync::Arc, time::Duration};

use rust_api_skeleton::cache::Cache;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct PublicSettings {
    currency: String,
}

async fn public_settings(cache: Arc<Cache>) -> anyhow::Result<PublicSettings> {
    cache
        .remember("public-settings:v1", Some(Duration::from_secs(60)), || async {
            // Ganti dengan pembacaan repository atau konfigurasi domain.
            Ok::<_, anyhow::Error>(PublicSettings { currency: "IDR".into() })
        })
        .await
}
```

`get`, `put`, `forget`, dan `ping` mengembalikan error yang disanitasi jika Redis
gagal, timeout, atau data tidak valid. `remember` memperlakukan kegagalan cache
sebagai miss, memanggil loader sekali untuk panggilan tersebut, lalu mencoba
menyimpan hasilnya. Error dari loader tetap dikembalikan dan tidak disimpan.
Kegagalan write cache tidak mengubah hasil loader yang berhasil.

Timeout berlaku pada operasi backend cache, bukan durasi loader. `remember`
dapat melakukan read dan write terpisah; masing-masing memiliki batas timeout.
Deadline request atau job tetap diatur oleh pemanggilnya.

Semua write memiliki TTL positif, maksimum 30 hari. Override memakai `Duration`
minimal 1 ms dan resolusi Redis milidetik. Value terlalu besar ditolak sebelum write dan
dibatasi pada sisi Redis saat read. Tidak ada API `FLUSHDB`/`FLUSHALL`.

## Konsistensi dan invalidation

Service harus memilih data yang memang boleh stale selama TTL. Setelah transaksi
perubahan data berhasil commit, panggil `forget` untuk key yang terdampak.
Invalidation gagal/timeout dapat meninggalkan data lama sampai expiry. Write yang
timeout juga mungkin sudah diterapkan Redis; timeout tidak membuktikan rollback.

`remember` tidak menyediakan lock, single-flight, atau cache stampede protection.
Dua request yang miss bersamaan dapat menjalankan loader masing-masing. Request
baca yang berbarengan dengan update juga dapat menulis ulang hasil lama setelah
invalidation. Jika data memerlukan konsistensi kuat, gunakan pembacaan database
atau desain key/version dan invalidation yang sesuai aturan domain.

Skeleton tidak otomatis menyimpan user, role, password hash, atau JWT ke Redis.
Guard autentikasi tetap memeriksa user/role di database. Jangan memakai cache
opsional sebagai satu-satunya sumber keputusan authorization, saldo, atau status
transaksi.

## Redis untuk worker

Worker dapat menggunakan Redis sebagai cache dengan menerima facade `Cache` pada
handler yang membutuhkannya. Library cache dapat dipakai oleh semua binary, tetapi
bootstrap worker bawaan belum menginjeksi cache ke welcome-email handler.

Queue dapat dipilih dengan `QUEUE_DRIVER=database|redis`, terpisah dari
`CACHE_DRIVER`. Driver Redis memakai **Apalis dan transactional outbox SQL**:
perubahan domain dan outbox dicatat bersama, relay menerbitkan ID setelah commit,
dan worker mengambil lease SQL sebelum menjalankan handler. Payload/hasil tetap
berada di SQL, sehingga delivery duplikat atau Redis yang kehilangan data dapat
dipulihkan. Idempotensi efek eksternal tetap menjadi tanggung jawab handler.

Queue menggunakan `QUEUE_REDIS_URL` dan `QUEUE_REDIS_PREFIX`; cache tetap memakai
`REDIS_URL` dan `CACHE_PREFIX`. Apalis mempunyai struktur antrean internal sendiri;
driver ini tidak menggunakan Redis Streams. `make serve` mengelola relay bersama
worker Redis secara default. Cara mengaktifkan, memisahkan relay, menguras backlog
lama, serta menguji kedua driver ada pada [panduan queue](queue.md).

Untuk produksi, pisahkan instance Redis cache dan queue. Cache boleh memakai
eviction seperti `allkeys-lru`; queue perlu kebijakan retensi/persistence dan
penanganan kapasitas yang sesuai agar job tidak terbuang. Nomor logical database
yang berbeda tetap berbagi batas memori dan eviction pada instance yang sama.
Lihat [dokumentasi eviction Redis](https://redis.io/docs/latest/develop/reference/eviction/).

## Pengujian

```bash
make docker-cache
TEST_REDIS_URL='redis://127.0.0.1:6379/0' make test-redis
```

Gunakan endpoint plaintext `redis://` pada Redis disposable. Akun test memerlukan
izin `ACL SETUSER` dan `ACL DELUSER`: suite membuat lalu menghapus satu user ACL
bernama acak untuk menguji pemulihan setelah kegagalan autentikasi pertama.
Hak administratif tersebut tidak diperlukan oleh akun aplikasi produksi.

Tests menggunakan namespace acak, menghapus key milik test, dan tidak melakukan
flush global. Fault proxy TCP lokal menguji pemulihan koneksi jaringan tanpa
restart Redis; proses test perlu dapat membuka listener loopback sementara.
Suite ini tidak menguji koneksi TLS nyata atau menjanjikan recovery dari semua
error server. Pengujian yang memerlukan Redis nyata ditandai ignored pada
perintah test biasa dan dijalankan secara eksplisit oleh target di atas. CI
menyediakan Redis 8.0 disposable dengan izin yang diperlukan; lihat hasil
workflow untuk memastikan status eksekusinya.
