//! Minimal starter reference for a new HTTP module.
//! This example deliberately avoids database access; real modules inject their
//! repository/service through AppState and follow docs/system-design-rules.md.
//! When these handlers are registered as public routes, update src/swagger/mod.rs
//! in the same change so OpenAPI matches the actual request/response contract.

use rust_api_skeleton::{
    domain::dtos::{Normalize, normalize_and_validate},
    errors::AppResult,
    response::{self, ApiCursorJson, ApiJson, ApiStatusJson},
    utils::validator::ValidatedJson,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateCustomerRequest {
    #[validate(length(
        min = 1,
        max = 100,
        message = "must contain 1 to 100 characters"
    ))]
    pub name: String,
    #[validate(email(message = "must be a valid email address"))]
    #[validate(length(max = 254, message = "must contain at most 254 characters"))]
    pub email: String,
}

impl Normalize for CreateCustomerRequest {
    fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        self.email = self.email.trim().to_lowercase();
    }
}

#[derive(Clone, Serialize)]
pub struct CustomerResponse {
    pub customer_id: Uuid,
    pub name: String,
    pub email: String,
}

#[derive(Clone, Default)]
pub struct CustomerService;

impl CustomerService {
    pub fn new() -> Self {
        Self
    }

    pub async fn create(&self, input: CreateCustomerRequest) -> AppResult<CustomerResponse> {
        let input = normalize_and_validate(input)?;
        Ok(CustomerResponse {
            customer_id: Uuid::now_v7(),
            name: input.name,
            email: input.email,
        })
    }

    pub async fn get(&self) -> AppResult<CustomerResponse> {
        Ok(sample_customer())
    }

    pub async fn list(&self) -> AppResult<(Vec<CustomerResponse>, Option<String>)> {
        Ok((vec![sample_customer()], None))
    }
}

pub async fn store(
    ValidatedJson(input): ValidatedJson<CreateCustomerRequest>,
) -> AppResult<ApiStatusJson<CustomerResponse>> {
    let customer = CustomerService::new().create(input).await?;
    Ok(response::created("Customer created", customer))
}

pub async fn show() -> AppResult<ApiJson<CustomerResponse>> {
    let customer = CustomerService::new().get().await?;
    Ok(response::success("Customer retrieved", customer))
}

pub async fn index() -> AppResult<ApiCursorJson<CustomerResponse>> {
    let (items, next_cursor) = CustomerService::new().list().await?;
    Ok(response::cursor(
        "Customers retrieved",
        items,
        next_cursor,
    ))
}

fn sample_customer() -> CustomerResponse {
    CustomerResponse {
        customer_id: Uuid::now_v7(),
        name: "Example Customer".into(),
        email: "customer@example.com".into(),
    }
}

fn main() {}
