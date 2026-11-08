# Standar error handling dan diagnostic logging

Dokumen ini berlaku untuk semua aplikasi turunan Rust API Skeleton. Source of truth: `src/errors/app_error.rs`, `src/utils/response.rs`, dan `src/logging/mod.rs`.

## Kontrak API

Seluruh kegagalan HTTP dipetakan dari `AppError` ke response envelope berikut:

```json
{
  "success": false,
  "code": "VALIDATION_FAILED",
  "message": "Validation failed",
  "errors": {
    "email": [{"code": "email", "message": "must be a valid email address"}]
  }
}
```

`errors` hanya muncul jika mempunyai detail validasi yang aman. Tidak ada field `data` pada failure. Response harus mempertahankan HTTP status dan header terkait, misalnya `WWW-Authenticate`, `Retry-After`. `x-request-id` sudah dibuat dan diteruskan oleh middleware HTTP untuk korelasi lintas layanan.

| HTTP | Stable code | Makna |
|---|---|---|
| 400 | BAD_REQUEST | JSON, path, query, atau cursor tidak valid |
| 401 | UNAUTHORIZED | Kredensial tidak valid |
| 403 | FORBIDDEN | Tidak memiliki izin |
| 404 | NOT_FOUND | Resource/route tidak ditemukan |
| 405 | METHOD_NOT_ALLOWED | Method tidak didukung |
| 408 | REQUEST_TIMEOUT | Deadline request |
| 409 | CONFLICT | Konflik operasi/data |
| 413 | PAYLOAD_TOO_LARGE | Body melebihi batas |
| 415 | UNSUPPORTED_MEDIA_TYPE | Format request tidak didukung |
| 422 | VALIDATION_FAILED | Validasi DTO gagal |
| 429 | RATE_LIMITED | Rate limit |
| 503 | SERVICE_UNAVAILABLE | Dependensi/kapasitas tidak siap |
| 500 | INTERNAL_ERROR | Internal/SQL error |

Perubahan `code` adalah breaking change untuk integrasi klien. Jangan menggunakan string `message` sebagai flow control. Semua varian error baru harus memiliki HTTP mapping, code mapping, regression test, dan OpenAPI schema yang tepat.

## Diagnostik aman

Log internal memakai `tracing` terstruktur. Untuk `Database` log `error_class=database`, `error_code=INTERNAL_ERROR`, dan SQLSTATE bila tersedia. Untuk `Internal` log klasifikasi dan debug backtrace ketika filter mengizinkan. **Jangan** mencetak `{error:?}`, `{error:#}`, pesan SQL driver, `anyhow` chain, atau payload pengguna tanpa security review: detail seperti itu bisa menyimpan password, DSN, token, email, atau query SQL. Untuk penelusuran akar masalah, tambahkan kategori failure yang aman di layer service/repository dan span contextual *allowlist*, bukan membuka raw diagnostic secara global.

HTTP trace span memuat method, path tanpa query, dan request ID. `x-request-id` header response dapat diberikan kepada tim operasi untuk mencari log terkait; pastikan proxy/gateway mem-forward correlation header sesuai kebijakan trust. UUID request ID tidak membuktikan identitas pengguna. Korelasi log request dari worker membutuhkan job/trace context tersendiri dan tidak boleh mengubah label metrics menjadi high-cardinality.

## Konfigurasi

Development (local/test), untuk investigasi sementara:

```dotenv
APP_ENV=local
LOG_FORMAT=pretty
LOG_OUTPUT=stdout
RUST_LOG=rust_api_skeleton=debug,info,sqlx=warn,tower_http=info
```

Production:

```dotenv
APP_ENV=production
LOG_FORMAT=json
LOG_OUTPUT=stdout
RUST_LOG=info,sqlx=warn,tower_http=info
```

`APP_ENV` tidak otomatis mengubah `RUST_LOG`; operator menetapkan verbosity eksplisit. Production stdout dikirim ke collector seperti SigNoz/OTel Collector. File logging tetap didukung melalui `LOG_OUTPUT=file|both` dengan file per process, channel bounded, rotation, dan retention. Hati-hati shared volumes pada multi replica. Debug logging **tidak** pernah memberikan izin membocorkan raw secrets ataupun underlying driver text.

## Checklist troubleshooting

1. Catat UTC timestamp, HTTP status, `code`, dan `x-request-id`.
2. Cari trace span berisi request ID, kemudian kategorikan error `internal` atau `database`.
3. Periksa dashboard HTTP 5xx, readiness, SQLSTATE, dan kondisi dependency.
4. Reproduksi secara lokal/staging dengan data sintetis; tingkatkan verbosity hanya pada target aman, bukan global raw SQL.
5. Tambahkan regression test sesuai `docs/testing-standards.md` sebelum rilis patch.

Catatan: logger tidak menyimpan entire underlying error chain karena risiko kebocoran. Bila incident membutuhkan detail lebih dalam, gunakan observability berizin dengan field allowlist dan audit akses; jangan menyalakan full-error logging production tanpa sanitasi.
