# Standar unit test, HTTP contract test, dan integration test

Berlaku untuk perubahan API, domain, service, repository, database, queue, scheduler, storage, dan generator. Tidak ada aturan “compile saja cukup”.

## Piramida test wajib

| Perubahan | Minimum test |
|---|---|
| Pure domain/validation/cursor/error mapping | `#[test]` unit, normal + batas + invalid |
| Async service/workflow | `#[tokio::test]` dengan fake trait repository, success + fail + cancellation bila relevan |
| HTTP handler/middleware/error | `tests/http_contract.rs`: method/status, JSON, code, header, auth, redaction |
| SQL/schema/transaction/index | Test PostgreSQL **dan** MySQL pada disposable database, rollback/migration, isolation, constraint dan query behavior |
| Queue/Redis/cache/outbox | Test retry, idempotency, lease/fencing, Redis integration, failure injection |
| CLI/generator | CLI/scaffold test: hasil file dan compile, validasi aturan schema |
| Perbaikan bug | Regression test yang menangkap bug asli sebelum patch |

Unit test dapat tinggal pada `#[cfg(test)] mod tests` di file modul. Integration test ada pada `tests/`. Gunakan `tests/support` untuk fixtures umum dan environment test. Jangan mengakses production database, shared Redis, atau data nyata. Hindari ketergantungan urutan test dan `sleep` arbitrer. Seluruh secrets test hanyalah dummy. Test tidak boleh mencetak kredensial.

## Nama, kualitas, dan isolasi

Gunakan nama `<behavior>_<expected_result>`, misalnya `internal_error_never_exposes_cause_to_client`. Setiap test harus menguji hasil dan side-effect yang relevan, bukan hanya `is_ok()`. Uji negative paths: invalid DTO, unauthorized, forbidden, duplicate, unavailable DB, timeout, retries, concurrency, pagination, batas body. Gunakan tabel kasus bila input/expected output seragam.

Test database memakai `TEST_POSTGRES_URL`, `TEST_MYSQL_URL`, atau fallback `TEST_DATABASE_URL` (hanya untuk satu matrix driver). Test `#[ignore]` untuk external services **hanya** dianggap lulus jika dijalankan secara eksplisit dan hasilnya hijau. Jangan menyimpulkan lulus dari `cargo test --all-targets` saja.

## Perintah baseline

```bash
make check
make test-scaffolds
```

`make check` memvalidasi formatting, Clippy, unit+HTTP contract/doc tests, dan feature matrix PostgreSQL-only/MySQL-only. `make precommit` menjalankan baseline dan test scaffold.

Ketika mengubah persistence/auth/queue, wajib jalankan pada database/Redis disposable yang sesuai:

```bash
make test-postgres
make test-mysql
make test-redis
make test-redis-transport
make test-queue-postgres
make test-queue-mysql
```

Perintah di atas bukan semuanya wajib untuk setiap perubahan. Perubahan persistence memerlukan matrix PostgreSQL/MySQL; perubahan Redis cache memerlukan Redis cache; perubahan transport/outbox Redis memerlukan transport dan queue integration pada kedua driver. Seluruh dependency external yang diperlukan harus tersedia. `make doctor` untuk preflight sebelum release ketika dependency tersedia.

## Coverage dan acceptance

Tidak menetapkan angka coverage semu pada skeleton generik; coverage line saja tidak membuktikan correctness. Untuk modul baru, seluruh aturan domain dan semua jalur failure yang teridentifikasi wajib diuji, bersama status/response contract HTTP dan invariants security. Tambahkan threshold coverage per proyek turunan berdasarkan criticality, bukan menyembunyikan test gagal untuk memenuhi persentase. Perubahan terhadap format response/error harus diperiksa terhadap OpenAPI pada commit yang sama.

Pull request wajib mencantumkan: daftar tests yang ditambah, command yang berhasil, command yang tidak dapat dijalankan dan alasan, impact PostgreSQL/MySQL/Redis, dan apakah terdapat perubahan kompatibilitas. Semua quality gate wajib hijau sebelum merge; test yang skipped/ignored harus dicatat sebagai *not executed*, bukan lulus.

## Contoh unit test

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_business_identifier() {
        let value = "   ".trim();
        assert!(value.is_empty());
    }
}
```

Untuk test service gunakan fake repository yang mengimplementasikan trait dependency. Jangan membuat service unit test bergantung pada live DB; uji SQL secara nyata pada integration test terpisah.
