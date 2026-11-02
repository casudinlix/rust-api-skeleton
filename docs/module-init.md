# Starter pattern module baru

Dokumen ini adalah reference implementation untuk module HTTP baru setelah
`make install`. Source yang dapat dikompilasi ada di
[`examples/module_init.rs`](../examples/module_init.rs). Contoh tersebut sengaja
tidak memakai database supaya fokus pada boundary request, service, dan response;
module real tetap wajib mengikuti desain query, migration, repository, serta
indexing di [system-design-rules.md](system-design-rules.md).

## Alur standar

Gunakan generator sebagai titik awal:

```bash
make migration NAME=create_customers_table
make model NAME=Customer
make request NAME=CreateCustomerRequest
make repository NAME=CustomerRepository
make service NAME=CustomerService
make controller NAME=CustomerController
```

Urutan runtime yang diharapkan:

```text
Route
  -> Handler
     -> ValidatedJson<Request>
        -> Service
           -> normalize_and_validate(Request)
           -> Repository
        -> crate::response helper
```

Handler menangani transport HTTP dan authorization. Service memegang use case dan
memvalidasi ulang request supaya caller non-HTTP tidak dapat melewati aturan.
Repository memegang SQL parameterized pada adapter PostgreSQL/MySQL.

## Request validation

Request action wajib terpisah dari model persistence:

```rust
#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateCustomerRequest {
    #[validate(length(
        min = 1,
        max = 100,
        message = "must contain 1 to 100 characters"
    ))]
    pub name: String,
}
```

Implementasikan `Normalize` untuk text yang memang boleh dinormalisasi.
Password, token, signature, dan secret tidak boleh di-trim atau case-fold.
Handler memakai `ValidatedJson<T>`, sedangkan service memanggil
`normalize_and_validate(input)`.

## Global response helper

Semua module HTTP memakai facade yang sama:

```rust
use crate::response::{self, ApiCursorJson, ApiJson, ApiStatusJson};
```

Success biasa:

```rust
pub async fn show() -> AppResult<ApiJson<CustomerResponse>> {
    let customer = service.get(customer_id).await?;
    Ok(response::success("Customer retrieved", customer))
}
```

Create:

```rust
pub async fn store() -> AppResult<ApiStatusJson<CustomerResponse>> {
    let customer = service.create(input).await?;
    Ok(response::created("Customer created", customer))
}
```

Cursor list:

```rust
pub async fn index() -> AppResult<ApiCursorJson<CustomerResponse>> {
    let page = service.list(query).await?;
    Ok(response::cursor(
        "Customers retrieved",
        page.items,
        page.next_cursor,
    ))
}
```

Untuk status success lain gunakan `response::success_with_status`.
Failure dikembalikan sebagai `AppError`; jangan memanggil
`response::error` langsung dari handler kecuali sedang mengimplementasikan
infrastruktur error global.

## Batas arsitektur

Dilarang membawa `ApiJson`, `Json`, `StatusCode`, atau helper
`crate::response` ke service/domain. Mereka adalah concern HTTP. Response DTO
boleh berada di domain/dtos karena hanya data serializable; transport wrapper
tetap di handler.

Untuk list, service mengembalikan page internal:

```text
items + next_cursor
```

Handler yang mengubahnya menjadi `response::cursor`. Dengan begitu semua domain
mendapat bentuk API yang identik tanpa membuat pagination envelope sendiri.

## Swagger/OpenAPI untuk route baru

Jika module menambah endpoint publik, update `src/swagger/mod.rs` pada change
yang sama. Tambahkan path/method, `operationId` stabil, request/response schema,
status error aktual, dan `bearerAuth` bila route protected.

Contoh customer detail memakai `customers.show` dan
`#/components/schemas/CustomerEnvelope`. Response schema tetap harus mengikuti
`response::success(...)`; list harus mengikuti `response::cursor(...)`.
Jangan menambahkan field dokumentasi yang tidak ada pada DTO runtime. Lihat
[swagger.md](swagger.md) untuk contoh lengkap.

## Bila module menyimpan file

Gunakan storage global dari `AppState`:

```rust
use crate::filesystem::StorageDisk;

state
    .storage
    .put(
        StorageDisk::Private,
        "imports/customer.csv",
        &bytes,
    )
    .await?;
```

File public menggunakan `StorageDisk::Public`; jangan menulis langsung ke
`public/storage`. Untuk log domain gunakan structured target:

```rust
tracing::info!(
    target: "app::customer",
    event = "customer_imported",
    customer_id = %customer_id,
    "customer imported"
);
```

Jangan log raw upload, password, token, atau payload request. Detail filesystem,
`storage:link`, static assets, rotation, dan retention ada di
[filesystem.md](filesystem.md).

## Persistence rules tetap berlaku

Starter sample bukan izin untuk melewati aturan data. Sebelum repository real
ditulis:

- PK wajib `<entity>_id` dan UUIDv7.
- Query wajib menyebut kolom, tanpa `SELECT *`.
- Tidak boleh N+1.
- Pagination memakai cursor/keyset, bukan OFFSET.
- Query, constraint, dan indeks dirancang di migration sebelum fitur.
- PostgreSQL dan MySQL diverifikasi terpisah.
- Query plan penting diuji pada data representatif.

Gunakan User/Auth/Health bawaan sebagai contoh implementasi runtime yang nyata,
dan `examples/module_init.rs` sebagai contoh minimal untuk memulai module baru.
