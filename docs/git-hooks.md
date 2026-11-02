# Git pre-commit gate

Repository menyimpan hook resmi di:

```text
.githooks/pre-commit
```

Aktifkan sekali setelah clone:

```bash
make hooks-install
```

Perintah tersebut mengatur repository-local:

```text
core.hooksPath=.githooks
```

Jika repository sudah memakai custom `core.hooksPath`, installer **menolak**
menggantinya. Setelah meninjau konflik tersebut, penggantian eksplisit:

```bash
make hooks-install FORCE=1
```

Hook menjalankan:

```bash
git diff --cached --check
make precommit
```

`make precommit` menjalankan `make check` dan `make test-scaffolds`.
Ia tidak membutuhkan database atau Redis eksternal. Pemeriksaan dependency runtime
berada di `make doctor` dan release matrix.

Git masih menyediakan `git commit --no-verify`. Itu escape hatch Git, bukan
workflow normal. Jika digunakan untuk insiden yang sah, quality gate harus
dijalankan sebelum merge/release dan alasan bypass harus jelas pada review.
