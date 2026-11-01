//! Upgrade regression: immutable migration history and existing UUIDs survive
//! canonical PK names. Each test creates and removes only its own database.

use std::{borrow::Cow, collections::BTreeMap};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use rust_api_skeleton::{
    config::{
        QueueConfig,
        database::{DatabaseConfig, DatabaseDriver},
    },
    database::DatabasePool,
    jobs::{DatabaseQueue, ReservedJob},
};
use serde_json::{Value, json};
#[cfg(feature = "mysql")]
use sqlx::Row;
use sqlx::{
    migrate::{MigrateDatabase, Migrator},
    types::Json,
};
use uuid::Uuid;

const ALIGNMENT_VERSION: i64 = 20261102000000;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for disposable PostgreSQL with CREATE DATABASE"]
async fn postgres_schema_alignment() -> Result<()> {
    isolated(DatabaseDriver::Postgres).await
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for disposable MySQL 8 with CREATE DATABASE"]
async fn mysql_schema_alignment() -> Result<()> {
    isolated(DatabaseDriver::Mysql).await
}

async fn isolated(driver: DatabaseDriver) -> Result<()> {
    let supplied = std::env::var("TEST_DATABASE_URL").context("set TEST_DATABASE_URL")?;
    let mut url = url::Url::parse(&supplied).context("invalid TEST_DATABASE_URL")?;
    match driver {
        DatabaseDriver::Postgres => ensure!(
            matches!(url.scheme(), "postgres" | "postgresql"),
            "PostgreSQL URL required"
        ),
        DatabaseDriver::Mysql => ensure!(url.scheme() == "mysql", "MySQL URL required"),
    }
    url.set_path(&format!("schema_test_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::create_database(&url).await?,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::create_database(&url).await?,
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!("database driver is not compiled"),
    }
    let outcome = match DatabasePool::connect(&DatabaseConfig {
        driver,
        url: url.clone(),
        max_connections: 3,
        min_connections: 0,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 30,
    })
    .await
    {
        Ok(database) => {
            let result = exercise(&database, driver).await;
            database.close().await;
            result
        }
        Err(error) => Err(error),
    };
    let cleanup = match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::drop_database(&url).await,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::drop_database(&url).await,
        #[allow(unreachable_patterns)]
        _ => unreachable!("feature checked before database creation"),
    };
    outcome?;
    cleanup.context("remove isolated schema-upgrade database")
}

fn baseline_migrator(driver: DatabaseDriver) -> Migrator {
    let mut migrator = match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::migrate!("./migrations/postgres"),
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::migrate!("./migrations/mysql"),
        #[allow(unreachable_patterns)]
        _ => unreachable!("feature checked before database creation"),
    };
    migrator.migrations = Cow::Owned(
        migrator
            .iter()
            .filter(|migration| migration.version < ALIGNMENT_VERSION)
            .cloned()
            .collect(),
    );
    migrator
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    users: Vec<Json<Value>>,
    jobs: Vec<Json<Value>>,
}

struct Fixtures {
    user_ids: [Uuid; 2],
    job_ids: [Uuid; 4],
    lease_token: Uuid,
    timestamp: DateTime<Utc>,
}

impl Fixtures {
    fn new() -> Result<Self> {
        Ok(Self {
            user_ids: [Uuid::new_v4(), Uuid::now_v7()],
            job_ids: [
                Uuid::new_v4(),
                Uuid::now_v7(),
                Uuid::new_v4(),
                Uuid::now_v7(),
            ],
            lease_token: Uuid::new_v4(),
            timestamp: DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123456Z")?
                .with_timezone(&Utc),
        })
    }
}

async fn exercise(database: &DatabasePool, driver: DatabaseDriver) -> Result<()> {
    let baseline = baseline_migrator(driver);
    ensure!(
        baseline.iter().count() == 3,
        "unexpected historical baseline"
    );
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => baseline.run(pool).await?,
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => baseline.run(pool).await?,
    }
    ensure!(database.ready().await.is_err(), "old schema reported ready");
    let fixtures = Fixtures::new()?;
    seed_legacy(database, &fixtures).await?;
    let before = snapshot(database, false).await?;
    let old_ledger = ledger(database).await?;

    database.migrate().await?;
    database.migrate().await?;
    database.ready().await?;
    let after = snapshot(database, true).await?;
    ensure!(before == after, "upgrade changed existing application data");
    for (index, user_id) in fixtures.user_ids.iter().enumerate() {
        ensure!(user_id.get_version_num() == if index == 0 { 4 } else { 7 });
        ensure!(
            after
                .users
                .iter()
                .any(|row| row.0[0] == user_id.to_string()),
            "historical user identity was replaced"
        );
    }
    let current_ledger = ledger(database).await?;
    ensure!(
        current_ledger
            .iter()
            .filter(|(version, _, _)| *version < ALIGNMENT_VERSION)
            .cloned()
            .collect::<Vec<_>>()
            == old_ledger,
        "historical migration checksums or status changed"
    );
    ensure!(
        current_ledger
            .iter()
            .any(|(version, success, _)| *version == ALIGNMENT_VERSION && *success),
        "alignment migration is not recorded as applied"
    );
    assert_schema(database).await?;
    assert_constraints(database, &fixtures).await?;
    ensure!(
        after == snapshot(database, true).await?,
        "failed constraint statements modified a fixture"
    );
    assert_generated_claim_time(database, &fixtures).await?;
    assert_legacy_job_processing(database, &fixtures).await?;
    assert_new_v7_insert(database).await?;
    Ok(())
}

async fn seed_legacy(database: &DatabasePool, fixtures: &Fixtures) -> Result<()> {
    // These inserts intentionally use the historical id columns. They are
    // upgrade fixtures, not examples of creating new application identities.
    for (index, user_id) in fixtures.user_ids.iter().enumerate() {
        let email = format!("legacy-{index}@example.invalid");
        match database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                sqlx::query("INSERT INTO users (id,name,email,password_hash,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6)")
                    .bind(user_id).bind("Legacy user").bind(&email)
                    .bind("preserve-this-test-hash").bind(fixtures.timestamp)
                    .bind(fixtures.timestamp + Duration::microseconds(7)).execute(pool).await?;
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                sqlx::query("INSERT INTO users (id,name,email,password_hash,created_at,updated_at) VALUES (?,?,?,?,?,?)")
                    .bind(user_id.to_string()).bind("Legacy user").bind(&email)
                    .bind("preserve-this-test-hash").bind(fixtures.timestamp.naive_utc())
                    .bind((fixtures.timestamp + Duration::microseconds(7)).naive_utc()).execute(pool).await?;
            }
        }
    }
    for (index, status) in ["pending", "running", "completed", "failed"]
        .iter()
        .enumerate()
    {
        let job_id = fixtures.job_ids[index];
        let payload = Json(json!({"user_id": fixtures.user_ids[index % 2]}));
        let deduplication_key = format!("schema:{job_id}");
        let locked_until =
            (*status == "running").then_some(fixtures.timestamp + Duration::hours(1));
        let lock_token = locked_until.map(|_| fixtures.lease_token);
        let last_error = (*status == "failed").then_some("fixture_failure");
        match database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                sqlx::query("INSERT INTO jobs (id,kind,payload,deduplication_key,status,attempts,available_at,locked_until,lock_token,last_error,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
                    .bind(job_id).bind("send_welcome_email").bind(&payload).bind(&deduplication_key)
                    .bind(status).bind(1_i64).bind(fixtures.timestamp).bind(locked_until).bind(lock_token)
                    .bind(last_error).bind(fixtures.timestamp).bind(fixtures.timestamp + Duration::microseconds(7))
                    .execute(pool).await?;
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                sqlx::query("INSERT INTO jobs (id,kind,payload,deduplication_key,status,attempts,available_at,locked_until,lock_token,last_error,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
                    .bind(job_id.to_string()).bind("send_welcome_email").bind(&payload).bind(&deduplication_key)
                    .bind(status).bind(1_i64).bind(fixtures.timestamp.naive_utc()).bind(locked_until.map(|time| time.naive_utc()))
                    .bind(lock_token.map(|token| token.to_string())).bind(last_error)
                    .bind(fixtures.timestamp.naive_utc()).bind((fixtures.timestamp + Duration::microseconds(7)).naive_utc())
                    .execute(pool).await?;
            }
        }
    }
    Ok(())
}

async fn snapshot(database: &DatabasePool, canonical: bool) -> Result<Snapshot> {
    // Only internal, fixed identifiers enter these projections; data is never
    // interpolated. Every preserved column is enumerated explicitly.
    let user_pk = if canonical { "user_id" } else { "id" };
    let job_pk = if canonical { "job_id" } else { "id" };
    let user_columns = format!("{user_pk},name,email,password_hash,role,created_at,updated_at");
    let job_columns = format!(
        "{job_pk},kind,payload,deduplication_key,status,attempts,available_at,locked_until,lock_token,last_error,created_at,updated_at"
    );
    Ok(match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => Snapshot {
            users: sqlx::query_scalar(&format!(
                "SELECT jsonb_build_array({user_columns}) AS snapshot FROM users ORDER BY {user_pk}"
            ))
            .fetch_all(pool)
            .await?,
            jobs: sqlx::query_scalar(&format!(
                "SELECT jsonb_build_array({job_columns}) AS snapshot FROM jobs ORDER BY {job_pk}"
            ))
            .fetch_all(pool)
            .await?,
        },
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => Snapshot {
            users: sqlx::query_scalar(&format!(
                "SELECT JSON_ARRAY({user_columns}) AS snapshot FROM users ORDER BY {user_pk}"
            ))
            .fetch_all(pool)
            .await?,
            jobs: sqlx::query_scalar(&format!(
                "SELECT JSON_ARRAY({job_columns}) AS snapshot FROM jobs ORDER BY {job_pk}"
            ))
            .fetch_all(pool)
            .await?,
        },
    })
}

async fn ledger(database: &DatabasePool) -> Result<Vec<(i64, bool, Vec<u8>)>> {
    let sql = "SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version";
    Ok(match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => sqlx::query_as(sql).fetch_all(pool).await?,
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => sqlx::query_as(sql).fetch_all(pool).await?,
    })
}

async fn assert_schema(database: &DatabasePool) -> Result<()> {
    let primary_keys: Vec<(String, String)> = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let obsolete: i64 = sqlx::query_scalar("SELECT COUNT(column_name) AS obsolete_columns FROM information_schema.columns WHERE table_schema='public' AND table_name IN ('users','jobs') AND column_name='id'")
                .fetch_one(pool).await?;
            ensure!(obsolete == 0, "generic id column remains");
            let columns: Vec<(String, String)> = sqlx::query_as("SELECT table_name,data_type FROM information_schema.columns WHERE table_schema='public' AND ((table_name='users' AND column_name='user_id') OR (table_name='jobs' AND column_name='job_id')) ORDER BY table_name")
                .fetch_all(pool).await?;
            ensure!(
                columns
                    == vec![
                        ("jobs".into(), "uuid".into()),
                        ("users".into(), "uuid".into())
                    ]
            );
            let indexes: BTreeMap<String, String> = sqlx::query_as::<_, (String, String)>("SELECT indexname,indexdef FROM pg_indexes WHERE schemaname='public' AND tablename IN ('users','jobs')")
                .fetch_all(pool).await?.into_iter().collect();
            ensure!(
                indexes["users_created_at_user_id_index"]
                    .contains("(created_at DESC, user_id DESC)")
            );
            ensure!(indexes["jobs_claimable_idx"].contains("(claimable_at, created_at, job_id)"));
            ensure!(indexes["jobs_claimable_idx"].contains("WHERE (claimable_at IS NOT NULL)"));
            ensure!(indexes["jobs_retention_idx"].contains("(status, updated_at, job_id)"));
            ensure!(
                !indexes.contains_key("jobs_ready_idx")
                    && !indexes.contains_key("jobs_expired_idx")
            );
            sqlx::query_as("SELECT tc.table_name,kcu.column_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON tc.constraint_name=kcu.constraint_name AND tc.constraint_schema=kcu.constraint_schema WHERE tc.table_schema='public' AND tc.constraint_type='PRIMARY KEY' AND tc.table_name IN ('users','jobs','job_outbox') ORDER BY tc.table_name,kcu.ordinal_position")
                .fetch_all(pool).await?
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let obsolete: i64 = sqlx::query_scalar("SELECT COUNT(column_name) AS obsolete_columns FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name IN ('users','jobs') AND column_name='id'")
                .fetch_one(pool).await?;
            ensure!(obsolete == 0, "generic id column remains");
            let columns = sqlx::query("SELECT column_type AS column_type,character_set_name AS character_set_name,collation_name AS collation_name FROM information_schema.columns WHERE table_schema=DATABASE() AND ((table_name='users' AND column_name='user_id') OR (table_name='jobs' AND column_name='job_id'))")
                .fetch_all(pool).await.context("inspect MySQL canonical PK column types")?;
            ensure!(columns.len() == 2);
            for row in &columns {
                ensure!(mysql_metadata_text(row, "column_type")? == "char(36)");
                ensure!(mysql_metadata_text(row, "character_set_name")? == "ascii");
                ensure!(mysql_metadata_text(row, "collation_name")? == "ascii_bin");
            }
            let rows = sqlx::query("SELECT index_name AS index_name,column_name AS column_name,collation AS collation FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name IN ('users','jobs') ORDER BY table_name,index_name,seq_in_index")
                .fetch_all(pool).await.context("inspect MySQL cursor and queue indexes")?;
            let mut indexes = BTreeMap::<String, Vec<(String, String)>>::new();
            for row in rows {
                indexes
                    .entry(mysql_metadata_text(&row, "index_name")?)
                    .or_default()
                    .push((
                        mysql_metadata_text(&row, "column_name")?,
                        mysql_metadata_text(&row, "collation")?,
                    ));
            }
            ensure!(
                indexes["users_created_at_user_id_index"]
                    == vec![
                        ("created_at".into(), "D".into()),
                        ("user_id".into(), "D".into())
                    ]
            );
            ensure!(
                indexes["jobs_claimable_idx"]
                    == vec![
                        ("claimable_at".into(), "A".into()),
                        ("created_at".into(), "A".into()),
                        ("job_id".into(), "A".into())
                    ]
            );
            ensure!(
                indexes["jobs_retention_idx"]
                    == vec![
                        ("status".into(), "A".into()),
                        ("updated_at".into(), "A".into()),
                        ("job_id".into(), "A".into())
                    ]
            );
            ensure!(
                !indexes.contains_key("jobs_ready_idx")
                    && !indexes.contains_key("jobs_expired_idx")
            );
            sqlx::query("SELECT table_name AS table_name,column_name AS column_name FROM information_schema.key_column_usage WHERE table_schema=DATABASE() AND constraint_name='PRIMARY' AND table_name IN ('users','jobs','job_outbox') ORDER BY table_name,ordinal_position")
                .fetch_all(pool).await.context("inspect MySQL primary key names")?
                .iter()
                .map(|row| Ok((mysql_metadata_text(row, "table_name")?,mysql_metadata_text(row, "column_name")?)))
                .collect::<Result<Vec<_>>>()?
        }
    };
    ensure!(
        primary_keys
            == vec![
                ("job_outbox".into(), "job_outbox_id".into()),
                ("jobs".into(), "job_id".into()),
                ("users".into(), "user_id".into()),
            ]
    );
    Ok(())
}

#[cfg(feature = "mysql")]
fn mysql_metadata_text(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<String> {
    // INFORMATION_SCHEMA can mark textual metadata as BINARY/BLOB on the wire,
    // just like our ascii_bin UUID columns. Decode bytes and validate UTF-8.
    String::from_utf8(row.try_get::<Vec<u8>, _>(column)?)
        .with_context(|| format!("invalid UTF-8 in MySQL metadata column {column}"))
}

async fn assert_constraints(database: &DatabasePool, fixtures: &Fixtures) -> Result<()> {
    // Each statement uses its own autocommit transaction: an expected PostgreSQL
    // constraint failure must not poison later checks in an open transaction.
    let (duplicate_email, duplicate_pk, invalid_lease, generated_write) = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => (
            sqlx::query("UPDATE users SET email=$1 WHERE user_id=$2")
                .bind("legacy-0@example.invalid")
                .bind(fixtures.user_ids[1])
                .execute(pool)
                .await
                .map(|_| ()),
            sqlx::query("UPDATE users SET user_id=$1 WHERE user_id=$2")
                .bind(fixtures.user_ids[0])
                .bind(fixtures.user_ids[1])
                .execute(pool)
                .await
                .map(|_| ()),
            sqlx::query("UPDATE jobs SET status='running' WHERE job_id=$1")
                .bind(fixtures.job_ids[3])
                .execute(pool)
                .await
                .map(|_| ()),
            sqlx::query("UPDATE jobs SET claimable_at=$1 WHERE job_id=$2")
                .bind(fixtures.timestamp)
                .bind(fixtures.job_ids[0])
                .execute(pool)
                .await
                .map(|_| ()),
        ),
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => (
            sqlx::query("UPDATE users SET email=? WHERE user_id=?")
                .bind("legacy-0@example.invalid")
                .bind(fixtures.user_ids[1].to_string())
                .execute(pool)
                .await
                .map(|_| ()),
            sqlx::query("UPDATE users SET user_id=? WHERE user_id=?")
                .bind(fixtures.user_ids[0].to_string())
                .bind(fixtures.user_ids[1].to_string())
                .execute(pool)
                .await
                .map(|_| ()),
            sqlx::query("UPDATE jobs SET status='running' WHERE job_id=?")
                .bind(fixtures.job_ids[3].to_string())
                .execute(pool)
                .await
                .map(|_| ()),
            sqlx::query("UPDATE jobs SET claimable_at=? WHERE job_id=?")
                .bind(fixtures.timestamp.naive_utc())
                .bind(fixtures.job_ids[0].to_string())
                .execute(pool)
                .await
                .map(|_| ()),
        ),
    };
    for violation in [duplicate_email, duplicate_pk] {
        ensure!(
            violation.is_err_and(|error| error
                .as_database_error()
                .is_some_and(|db| db.is_unique_violation())),
            "unique constraint no longer enforced"
        );
    }
    ensure!(
        invalid_lease.is_err_and(|error| error
            .as_database_error()
            .is_some_and(|db| db.is_check_violation())),
        "running lease constraint no longer enforced"
    );
    ensure!(
        generated_write.is_err(),
        "generated due time accepted a direct write"
    );
    Ok(())
}

async fn claim_time(database: &DatabasePool, job_id: Uuid) -> Result<Option<NaiveDateTime>> {
    Ok(match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query_scalar(
                "SELECT claimable_at AT TIME ZONE 'UTC' AS claimable_at FROM jobs WHERE job_id=$1",
            )
            .bind(job_id)
            .fetch_one(pool)
            .await?
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query_scalar("SELECT claimable_at FROM jobs WHERE job_id=?")
                .bind(job_id.to_string())
                .fetch_one(pool)
                .await?
        }
    })
}

async fn assert_generated_claim_time(database: &DatabasePool, fixtures: &Fixtures) -> Result<()> {
    let available = fixtures.timestamp;
    let lease_expiry = available + Duration::hours(1);
    for (index, expected) in [Some(available), Some(lease_expiry), None, None]
        .into_iter()
        .enumerate()
    {
        ensure!(
            claim_time(database, fixtures.job_ids[index]).await?
                == expected.map(|time| time.naive_utc())
        );
    }
    // Exercise transitions on a historical UUIDv4 row, without rewriting its ID.
    for (status, expected) in [
        ("running", Some(lease_expiry)),
        ("pending", Some(available)),
        ("completed", None),
        ("failed", None),
    ] {
        let locked_until = (status == "running").then_some(lease_expiry);
        let lock_token = locked_until.map(|_| fixtures.lease_token);
        match database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                sqlx::query(
                    "UPDATE jobs SET status=$1,locked_until=$2,lock_token=$3 WHERE job_id=$4",
                )
                .bind(status)
                .bind(locked_until)
                .bind(lock_token)
                .bind(fixtures.job_ids[0])
                .execute(pool)
                .await?;
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                sqlx::query("UPDATE jobs SET status=?,locked_until=?,lock_token=? WHERE job_id=?")
                    .bind(status)
                    .bind(locked_until.map(|time| time.naive_utc()))
                    .bind(lock_token.map(|token| token.to_string()))
                    .bind(fixtures.job_ids[0].to_string())
                    .execute(pool)
                    .await?;
            }
        }
        ensure!(
            claim_time(database, fixtures.job_ids[0]).await?
                == expected.map(|time| time.naive_utc()),
            "generated due time did not follow {status}"
        );
    }
    Ok(())
}

async fn assert_legacy_job_processing(database: &DatabasePool, fixtures: &Fixtures) -> Result<()> {
    // Restore this fixture to pending after the generated-column transition
    // checks. Its original eligibility precedes the other running fixture.
    let legacy_job_id = fixtures.job_ids[0];
    ensure!(legacy_job_id.get_version_num() == 4);
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE jobs SET status='pending' WHERE job_id=$1")
                .bind(legacy_job_id)
                .execute(pool)
                .await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE jobs SET status='pending' WHERE job_id=?")
                .bind(legacy_job_id.to_string())
                .execute(pool)
                .await?;
        }
    }
    let queue = DatabaseQueue::new(
        database.clone(),
        QueueConfig {
            visibility_timeout_secs: 60,
            job_timeout_secs: 30,
            max_attempts: 3,
            ..QueueConfig::default()
        },
    );
    let job = queue
        .claim()
        .await?
        .context("historical UUIDv4 job became unclaimable")?;
    ensure!(job.job_id == legacy_job_id && job.job_id.get_version_num() == 4);
    ensure!(job.attempts == 2 && job.payload == json!({"user_id": fixtures.user_ids[0]}));
    let forged = ReservedJob {
        lock_token: Uuid::new_v4(),
        ..job.clone()
    };
    ensure!(
        !queue.ack(&forged).await?,
        "historical job lost lease fencing"
    );
    ensure!(queue.ack(&job).await?, "historical job could not finish");
    ensure!(
        !queue.ack(&job).await?,
        "historical job accepted a second acknowledgement"
    );
    ensure!(claim_time(database, legacy_job_id).await?.is_none());
    Ok(())
}

async fn assert_new_v7_insert(database: &DatabasePool) -> Result<()> {
    let user_id = Uuid::now_v7();
    let job_id = Uuid::now_v7();
    ensure!(user_id.get_version_num() == 7 && job_id.get_version_num() == 7);
    let payload = Json(json!({"user_id": user_id}));
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(
                "INSERT INTO users (user_id,name,email,password_hash) VALUES ($1,$2,$3,$4)",
            )
            .bind(user_id)
            .bind("New user")
            .bind("new-v7@example.invalid")
            .bind("new-test-hash")
            .execute(pool)
            .await?;
            sqlx::query("INSERT INTO jobs (job_id,kind,payload) VALUES ($1,$2,$3)")
                .bind(job_id)
                .bind("send_welcome_email")
                .bind(&payload)
                .execute(pool)
                .await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("INSERT INTO users (user_id,name,email,password_hash) VALUES (?,?,?,?)")
                .bind(user_id.to_string())
                .bind("New user")
                .bind("new-v7@example.invalid")
                .bind("new-test-hash")
                .execute(pool)
                .await?;
            sqlx::query("INSERT INTO jobs (job_id,kind,payload) VALUES (?,?,?)")
                .bind(job_id.to_string())
                .bind("send_welcome_email")
                .bind(&payload)
                .execute(pool)
                .await?;
        }
    }
    assert_pending_due_time(database, job_id, None).await?;

    // MySQL evaluates generated expressions before automatic timestamp defaults
    // as well as afterward. These are real prepared SQLx INSERTs: omitting the
    // value and writing DEFAULT must both produce the final exact due time.
    let default_job_id = Uuid::now_v7();
    let explicit_job_id = Uuid::now_v7();
    let explicit = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123456Z")?.with_timezone(&Utc);
    ensure!(default_job_id.get_version_num() == 7 && explicit_job_id.get_version_num() == 7);
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(
                "INSERT INTO jobs (job_id,kind,payload,available_at) VALUES ($1,$2,$3,DEFAULT)",
            )
            .bind(default_job_id)
            .bind("send_welcome_email")
            .bind(&payload)
            .execute(pool)
            .await?;
            sqlx::query("INSERT INTO jobs (job_id,kind,payload,available_at) VALUES ($1,$2,$3,$4)")
                .bind(explicit_job_id)
                .bind("send_welcome_email")
                .bind(&payload)
                .bind(explicit)
                .execute(pool)
                .await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(
                "INSERT INTO jobs (job_id,kind,payload,available_at) VALUES (?,?,?,DEFAULT)",
            )
            .bind(default_job_id.to_string())
            .bind("send_welcome_email")
            .bind(&payload)
            .execute(pool)
            .await?;
            sqlx::query("INSERT INTO jobs (job_id,kind,payload,available_at) VALUES (?,?,?,?)")
                .bind(explicit_job_id.to_string())
                .bind("send_welcome_email")
                .bind(&payload)
                .bind(explicit.naive_utc())
                .execute(pool)
                .await?;
        }
    }
    assert_pending_due_time(database, default_job_id, None).await?;
    assert_pending_due_time(database, explicit_job_id, Some(explicit.naive_utc())).await?;
    Ok(())
}

async fn assert_pending_due_time(
    database: &DatabasePool,
    job_id: Uuid,
    expected: Option<NaiveDateTime>,
) -> Result<()> {
    let (available, claimable): (NaiveDateTime, Option<NaiveDateTime>) = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => sqlx::query_as("SELECT available_at AT TIME ZONE 'UTC' AS available_at,claimable_at AT TIME ZONE 'UTC' AS claimable_at FROM jobs WHERE job_id=$1")
            .bind(job_id).fetch_one(pool).await?,
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => sqlx::query_as("SELECT available_at,claimable_at FROM jobs WHERE job_id=?")
            .bind(job_id.to_string()).fetch_one(pool).await?,
    };
    ensure!(
        claimable == Some(available),
        "pending due time differs from final availability"
    );
    ensure!(available.and_utc().timestamp_subsec_nanos() % 1_000 == 0);
    if let Some(expected) = expected {
        ensure!(
            available == expected,
            "explicit microsecond availability changed"
        );
    } else {
        ensure!(
            (Utc::now().naive_utc() - available).num_seconds().abs() < 30,
            "default availability is not current UTC time"
        );
    }
    Ok(())
}
