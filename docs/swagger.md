# Swagger / OpenAPI

Swagger bersifat **opsional** dan default **OFF**. Implementasi tidak menambah
crate baru: OpenAPI 3.1 dibangun dari kontrak eksplisit di `src/swagger/mod.rs`,
sedangkan Swagger UI memakai asset browser yang versinya dipin. API server tidak
melakukan HTTP request ke CDN.

## Konfigurasi

```dotenv
SWAGGER_ENABLED=false
SWAGGER_UI_PATH=/docs
SWAGGER_OPENAPI_PATH=/api-docs/openapi.json
SWAGGER_ASSET_BASE_URL=https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.32.6
```

Aktifkan:

```dotenv
SWAGGER_ENABLED=true
```

Kemudian:

```text
GET /docs
GET /api-docs/openapi.json
```

Jika `SWAGGER_ENABLED=false`, kedua route **tidak diregistrasikan** dan
menghasilkan kontrak 404 standar aplikasi. Setting UI/path/asset lain tidak
dibutuhkan untuk boot ketika Swagger disabled; validasi ketat diterapkan saat
Swagger diaktifkan.

`SWAGGER_UI_PATH` dan `SWAGGER_OPENAPI_PATH` harus berupa absolute route
yang aman, berbeda satu sama lain, dan tidak boleh berada di bawah `/api/v1`
atau menimpa `/health` / `/ready`.

`SWAGGER_ASSET_BASE_URL` wajib HTTPS tanpa credential, query, atau fragment.
Untuk environment tanpa Internet, arahkan ke mirror HTTPS internal yang menyajikan
`swagger-ui.css`, `swagger-ui-bundle.js`, dan
`swagger-ui-standalone-preset.js` dari versi yang sama.

## Security

Swagger UI tidak memberikan authorization baru. Endpoint bisnis tetap memakai
middleware/guard yang sama.

OpenAPI mendefinisikan:

```text
bearerAuth
type: http
scheme: bearer
bearerFormat: JWT
```

Swagger UI menggunakan `persistAuthorization: false`, jadi bearer token tidak
diminta untuk dipersist oleh UI.

Untuk production:

- default tetap `SWAGGER_ENABLED=false`;
- jika diaktifkan, batasi `/docs` dan `/api-docs/openapi.json` di reverse
  proxy/VPN/internal network bila kontrak API tidak dimaksudkan publik;
- jangan memasukkan JWT asli, password, API key, DSN, private key, email pribadi,
  atau secret provider ke example OpenAPI;
- jangan menganggap menyembunyikan Swagger sebagai pengganti authentication atau
  authorization endpoint.

## Kontrak yang didokumentasikan

Reference skeleton saat ini mendokumentasikan:

```text
GET    /health
GET    /ready
POST   /api/v1/auth/register
POST   /api/v1/auth/login
GET    /api/v1/me
PUT    /api/v1/me
GET    /api/v1/users
POST   /api/v1/users
GET    /api/v1/users/{user_id}
PUT    /api/v1/users/{user_id}
DELETE /api/v1/users/{user_id}
```

Schema mengikuti response global:

```json
{
  "success": true,
  "message": "User retrieved",
  "data": {}
}
```

Cursor mengikuti:

```json
{
  "success": true,
  "message": "Users retrieved",
  "data": [],
  "meta": {
    "next_cursor": null,
    "has_more": false
  }
}
```

Error tidak mendokumentasikan `data: null`, karena runtime memang tidak
mengirim field tersebut.

## Rule endpoint baru

Setiap endpoint publik yang ditambah atau diubah wajib memperbarui OpenAPI pada
change yang sama.

Minimal yang harus sinkron:

1. path dan HTTP method;
2. `operationId` unik dan stabil;
3. tag;
4. request body/query/path parameter;
5. success response **sesuai global response helper**;
6. error status yang memang dapat dikembalikan runtime;
7. bearer security pada endpoint protected;
8. cursor `next_cursor/has_more` untuk list;
9. schema field dan format aktual;
10. example palsu yang aman.

Dilarang mendokumentasikan field yang tidak benar-benar diterima atau dikirim,
termasuk `password_hash`, internal lock token, queue payload internal, DSN, atau
secret.

## Example: module Customer

Misalnya route nyata:

```text
GET /api/v1/customers/{customer_id}
```

Tambahkan path ke `openapi_document()`:

```rust
"/api/v1/customers/{customer_id}": {
    "parameters": [{
        "name": "customer_id",
        "in": "path",
        "required": true,
        "schema": {"type": "string", "format": "uuid"}
    }],
    "get": operation(
        "customers.show",
        "Get one customer",
        "customers",
        true,
        None,
        responses(&[
            ("200", "Customer retrieved", "#/components/schemas/CustomerEnvelope"),
            ("400", "Invalid UUID", "#/components/schemas/ErrorResponse"),
            ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
            ("404", "Customer not found", "#/components/schemas/ErrorResponse")
        ])
    )
}
```

Lalu tambahkan `CustomerResponse` dan `CustomerEnvelope` pada `schemas()`.
Schema harus berasal dari DTO/response runtime yang nyata, bukan rancangan yang
belum diimplementasikan.

Untuk list customer, dokumentasikan `cursor` dan `limit`, lalu response wajib
menggunakan bentuk cursor global. Jangan memperkenalkan `page` atau `offset`
hanya karena Swagger UI kebetulan membuat form query menjadi mudah.

## Mengubah Swagger implementation

Saat toolchain tersedia, project boleh bermigrasi ke generator seperti
`utoipa` bila memang mengurangi maintenance. Migrasi tersebut harus:

- mempertahankan runtime toggle dan route contract;
- memperbarui `Cargo.lock` secara reproducible;
- tidak membutuhkan network saat runtime API;
- tetap menguji bahwa route runtime dan OpenAPI tidak drift;
- mempertahankan schema security/secret rules di atas.

Saat ini implementasi dependency-free dipilih agar skeleton tidak mengubah
dependency graph tanpa kemampuan meregenerasi dan memverifikasi lockfile.
