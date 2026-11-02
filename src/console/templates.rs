//! Source templates for the developer console's generators.

pub(super) fn controller(entity: &str) -> String {
    r#"use serde_json::Value;

use crate::{
    errors::{AppError, AppResult},
    middleware::auth::AuthUser,
    response::ApiJson,
};

/// Inject the service through AppState, authorize access, then return its DTO.
/// Use crate::response::success/created/cursor for successful HTTP responses.
/// Reference examples/module_init.rs and docs/module-init.md for the starter pattern.
/// Register this handler in src/routes/api.rs after implementing the action.
/// Update src/swagger/mod.rs in the same change for every public endpoint contract.
pub async fn index(_auth: AuthUser) -> AppResult<ApiJson<Value>> {
    Err(AppError::Internal(anyhow::anyhow!(
        "Implement the __ENTITY__ controller before registering its routes"
    )))
}
"#
    .replace("__ENTITY__", entity)
}

pub(super) fn model(module: &str, entity: &str) -> String {
    r#"/// Add domain fields and map database rows in each driver-specific repository.
/// Expose a separate response DTO so internal fields are not serialized by accident.
/// Map persisted rows directly; constructing a new entity generates a new identity.
#[derive(Clone, Debug)]
pub struct __ENTITY__ {
    pub __MODULE___id: ::uuid::Uuid,
    pub created_at: ::chrono::DateTime<::chrono::Utc>,
    pub updated_at: ::chrono::DateTime<::chrono::Utc>,
}

impl __ENTITY__ {
    pub fn new() -> Self {
        let now = ::chrono::Utc::now();
        Self {
            __MODULE___id: ::uuid::Uuid::now_v7(),
            created_at: now,
            updated_at: now,
        }
    }
}

impl ::std::default::Default for __ENTITY__ {
    fn default() -> Self {
        Self::new()
    }
}
"#
    .replace("__MODULE__", module)
    .replace("__ENTITY__", entity)
}

pub(super) fn request(entity: &str) -> String {
    r#"use serde::Deserialize;
use validator::Validate;

use crate::domain::dtos::Normalize;

/// Laravel-style request boundary for one action.
/// - deny unknown fields to prevent mass assignment
/// - normalize text before validation
/// - keep secrets such as passwords byte-for-byte unchanged
/// - use ValidatedJson<Self> in the handler; services re-run normalize_and_validate
#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct __ENTITY__Request {
    #[validate(length(
        min = 1,
        max = 100,
        message = "must contain 1 to 100 characters"
    ))]
    pub name: String,
}

impl Normalize for __ENTITY__Request {
    fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
    }
}
"#
    .replace("__ENTITY__", entity)
}

pub(super) fn service(entity: &str) -> String {
    r#"use crate::errors::{AppError, AppResult};

/// Inject an Arc<dyn Repository> and other dependencies through the constructor.
#[derive(Clone, Default)]
pub struct __ENTITY__Service;

impl __ENTITY__Service {
    pub fn new() -> Self {
        Self
    }

    /// Replace this action with domain-specific input, authorization and output.
    pub async fn execute(&self) -> AppResult<()> {
        Err(AppError::Internal(anyhow::anyhow!(
            "Implement the __ENTITY__ service action before calling it"
        )))
    }
}
"#
    .replace("__ENTITY__", entity)
}

pub(super) fn repository(module: &str, entity: &str) -> String {
    r#"use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::AppResult;

/// Keep SQL and driver-specific types inside the PostgreSQL/MySQL adapters.
/// Extend this boundary with domain entities and the operations your service needs.
/// Separate public read projections from credentials; bound batch and cursor reads.
#[async_trait]
pub trait __ENTITY__Repository: Send + Sync {
    async fn exists(&self, __MODULE___id: Uuid) -> AppResult<bool>;
}
"#
    .replace("__MODULE__", module)
    .replace("__ENTITY__", entity)
}

pub(super) fn postgres_repository(module: &str, entity: &str) -> String {
    repository_adapter(module, entity, "PgPool", "Postgres", "PostgreSQL")
}

pub(super) fn mysql_repository(module: &str, entity: &str) -> String {
    repository_adapter(module, entity, "MySqlPool", "Mysql", "MySQL")
}

fn repository_adapter(
    module: &str,
    entity: &str,
    pool: &str,
    prefix: &str,
    driver: &str,
) -> String {
    r#"use async_trait::async_trait;
use sqlx::__POOL__;
use uuid::Uuid;

use crate::{
    errors::{AppError, AppResult},
    repositories::__MODULE___repository::__ENTITY__Repository,
};

#[derive(Clone)]
pub struct __PREFIX____ENTITY__Repository {
    // Rename to pool when implementing parameterized queries for this adapter.
    _pool: __POOL__,
}

impl __PREFIX____ENTITY__Repository {
    pub fn new(pool: __POOL__) -> Self {
        Self { _pool: pool }
    }
}

#[async_trait]
impl __ENTITY__Repository for __PREFIX____ENTITY__Repository {
    async fn exists(&self, ___MODULE___id: Uuid) -> AppResult<bool> {
        // Add dialect-specific SQL and bind every user-supplied value.
        // Name only required columns; design indexes and bounded queries first.
        Err(AppError::Internal(anyhow::anyhow!(
            "Implement the __DRIVER__ __ENTITY__ repository before using it"
        )))
    }
}
"#
    .replace("__POOL__", pool)
    .replace("__MODULE__", module)
    .replace("__ENTITY__", entity)
    .replace("__PREFIX__", prefix)
    .replace("__DRIVER__", driver)
}

pub(super) fn job(module: &str, entity: &str) -> String {
    r#"use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::{AppError, AppResult};

pub const KIND: &str = "__MODULE__";

/// Replace resource_id with the concrete entity key, such as invoice_id.
/// Carry only the minimum data needed to load current state.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct __ENTITY__Payload {
    pub resource_id: Uuid,
}

/// Make external effects idempotent: durable jobs can be delivered more than once.
/// Register KIND in src/jobs/worker.rs and map failures to a safe JobError category.
pub async fn handle(_payload: __ENTITY__Payload) -> AppResult<()> {
    Err(AppError::Internal(anyhow::anyhow!(
        "Implement the __ENTITY__ job before registering its worker dispatch"
    )))
}
"#
    .replace("__MODULE__", module)
    .replace("__ENTITY__", entity)
}

pub(super) fn middleware() -> String {
    r#"use axum::{extract::Request, middleware::Next, response::Response};

use crate::errors::{AppError, AppResult};

/// Define this middleware's policy, then attach it with middleware::from_fn.
/// Reject access until the policy is implemented; on success call next.run(request).
pub async fn handle(_request: Request, _next: Next) -> AppResult<Response> {
    Err(AppError::Forbidden)
}
"#
    .to_owned()
}

pub(super) fn migration(name: &str, driver: &str, counterpart: &str) -> String {
    format!(
        "-- {driver} migration: {name}\n\
         -- SCAFFOLD ONLY: write the forward migration SQL below.\n\
         -- Complete both this file and the {counterpart} counterpart before make migrate.\n\
         -- Applying this comment-only stub would record an empty migration.\n\
         -- Never modify an applied migration; create a new migration instead.\n\
         -- Design first: document this feature in docs/design/<feature>.md.\n\
         -- State each table-to-entity mapping explicitly: people.person_id, statuses.status_id.\n\
         -- Every application table needs a named <entity>_id primary key, including join tables.\n\
         -- New primary keys come from the application using Uuid::now_v7().\n\
         -- PostgreSQL UUID; MySQL CHAR(36) CHARACTER SET ascii COLLATE ascii_bin.\n\
         -- Preserve stored identifiers when upgrading existing tables.\n\
         -- Define FK/unique/check constraints and query-specific indexes before feature code.\n\
         -- Plan explicit minimal column projections, access scope and bounded relation batches.\n\
         -- Lists use versioned, scoped cursors with immutable sort keys and the PK tie-breaker.\n\
         -- Record query budgets and verify EXPLAIN plans with representative data on both drivers.\n\n"
    )
}
