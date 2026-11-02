# Observability dan process health

Skeleton menyediakan listener observability terpisah untuk setiap process. Listener
ini adalah concern infrastruktur, bukan bagian dari kontrak JSON bisnis di
`/api/v1`.

| Process | Default port | Endpoint |
|---|---:|---|
| API | 9100 | `/health`, `/ready`, `/metrics` |
| Worker | 9101 | `/health`, `/ready`, `/metrics` |
| Scheduler | 9102 | `/health`, `/ready`, `/metrics` |
| Relay | 9103 | `/health`, `/ready`, `/metrics` |

Default native binding adalah `127.0.0.1`. Compose mengubah
`OBSERVABILITY_HOST=0.0.0.0` agar collector di container network dapat melakukan
scrape, tetapi port tidak dipublish ke host. Jangan expose listener ini langsung
ke Internet.

## Konfigurasi

```dotenv
OBSERVABILITY_HOST=127.0.0.1
OBSERVABILITY_API_PORT=9100
OBSERVABILITY_WORKER_PORT=9101
OBSERVABILITY_SCHEDULER_PORT=9102
OBSERVABILITY_RELAY_PORT=9103
```

Keempat port harus nonzero, berbeda satu sama lain, dan tidak boleh menggunakan
`APP_PORT`. Jika menjalankan beberapa replica process dalam network namespace
yang sama, berikan port berbeda per replica. Container/pod terpisah dapat memakai
default yang sama karena mempunyai namespace jaringan sendiri.

## Semantik health dan readiness

`/health` hanya menunjukkan process observability masih hidup. Endpoint ini tidak
melakukan I/O dependency.

`/ready` mengembalikan 200 hanya jika:

1. database dapat membaca ledger migration yang persis sesuai binary; dan
2. loop/backend utama process telah mencapai kondisi siap.

Makna backend per process:

- API: listener HTTP bisnis berhasil bind.
- Worker database: acquisition loop sudah aktif.
- Worker Redis: koneksi/consumer Redis berhasil dibuat.
- Scheduler: cron scheduler berhasil start.
- Relay: koneksi Redis awal berhasil dibuat.

Saat shutdown dimulai, probe process diturunkan menjadi not-ready sebelum process
selesai drain. Database outage membuat seluruh process menjadi not-ready karena
readiness juga memeriksa migration ledger.

Readiness backend Redis bersifat **passive runtime signal**, bukan `PING` baru
pada setiap health request. Redis worker menurunkan readiness saat consumer
session berhenti dan masuk reconnect. Relay menunggu Redis sebelum menyatakan
ready. Pantau juga metric publish/reconnect serta Redis server sendiri untuk
deteksi dependency yang lebih lengkap.

## Metrics

`/metrics` memakai Prometheus text exposition dan tidak memerlukan crate metrics
tambahan. Registry hanya memakai counter/gauge bounded tanpa label resource,
user, route dinamis, tenant, job ID, atau nilai input.

Metric utama:

- `rust_api_process_ready`
- `rust_api_database_pool_connections{state="open|idle|active"}`
- `rust_api_http_requests_total`
- `rust_api_http_requests_in_flight`
- `rust_api_http_responses_total{state="2xx|3xx|4xx|5xx|other"}`
- `rust_api_http_request_duration_seconds_sum/count`
- `rust_api_worker_jobs_started_total`
- `rust_api_worker_jobs_in_flight`
- `rust_api_worker_jobs_completed_total`
- `rust_api_worker_job_failures_total`
- `rust_api_worker_lease_lost_total`
- `rust_api_worker_transition_errors_total`
- `rust_api_worker_job_duration_seconds_sum/count`
- `rust_api_queue_claim_errors_total`
- `rust_api_redis_reconnects_total`
- `rust_api_relay_published_total`
- `rust_api_relay_publish_failed_total`
- `rust_api_scheduler_enqueued_total`
- `rust_api_scheduler_enqueue_failed_total`

Duration diekspor sebagai `_sum` dan `_count`; backend monitoring dapat
menghitung rata-rata/rate. Skeleton tidak mengklaim histogram bucket universal
karena target latency setiap proyek berbeda.

Queue depth dan oldest-job-age tidak dihitung dengan query database pada setiap
scrape. Hal itu sengaja untuk menghindari query agregasi periodik yang menjadi
beban tersembunyi. Gunakan query diagnosis/indexed yang sudah ada di
[operations.md](operations.md), atau tambahkan collector/domain metric dengan
query budget dan indeks yang diverifikasi saat proyek nyata membutuhkannya.

## Prometheus / SigNoz

Contoh Prometheus untuk deployment yang dapat menjangkau listener internal:

```yaml
scrape_configs:
  - job_name: rust-api
    static_configs:
      - targets:
          - api:9100
          - worker:9101
          - scheduler:9102
```

Jika relay terpisah, tambahkan `relay:9103`.

SigNoz atau OpenTelemetry Collector dapat memakai Prometheus receiver untuk
mengambil endpoint yang sama. Structured logs tetap keluar melalui stdout dan
dapat dikumpulkan oleh agent/collector deployment. Skeleton tidak mengunci
aplikasi pada satu backend observability.

## Alert minimum

Baseline generik yang layak:

- process readiness = 0 selama beberapa interval;
- HTTP 5xx rate meningkat;
- DB active connections mendekati open/max pool;
- worker transition/claim errors meningkat;
- Redis reconnect atau relay publish failure meningkat;
- scheduler enqueue failure > 0;
- worker jobs in-flight macet lebih lama dari deadline normal.

Threshold numerik harus ditentukan dari workload proyek nyata, bukan ditanam
sebagai angka ajaib di skeleton.
