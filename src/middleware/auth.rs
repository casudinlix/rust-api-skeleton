use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};

use crate::{
    domain::models::user::User,
    errors::{AppError, AppResult},
    state::AppState,
};

pub struct AuthUser(pub User);

pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> AppResult<Self> {
        let token = bearer_token(&parts.headers)?;
        state.auth_service.authenticate(token).await.map(Self)
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> AppResult<Self> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        require_admin(user)
    }
}

fn require_admin(user: User) -> AppResult<AdminUser> {
    if user.role == "admin" {
        Ok(AdminUser(user))
    } else {
        Err(AppError::Forbidden)
    }
}

fn bearer_token(headers: &HeaderMap) -> AppResult<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next().ok_or(AppError::Unauthorized)?;
    if values.next().is_some() {
        return Err(AppError::Unauthorized);
    }
    let value = value.to_str().map_err(|_| AppError::Unauthorized)?;
    let (scheme, token) = value.split_once(' ').ok_or(AppError::Unauthorized)?;
    // Scheme names are case-insensitive. The credential must be a single compact
    // token: no lists, tabs, surrounding whitespace, or repeated separators.
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AppError::Unauthorized);
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;
    use crate::services::test_support::fixture_user;

    #[test]
    fn accepts_one_case_insensitive_bearer_token() {
        for scheme in ["Bearer", "bearer", "BEARER"] {
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                format!("{scheme} aaa.bbb.ccc").parse().unwrap(),
            );
            assert_eq!(bearer_token(&headers).unwrap(), "aaa.bbb.ccc");
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_malformed_bearer_credentials() {
        assert!(matches!(
            bearer_token(&HeaderMap::new()),
            Err(AppError::Unauthorized)
        ));
        for value in [
            "",
            "Bearer",
            "Bearer ",
            "Bearer  token",
            " Bearer token",
            "Bearer token ",
            "Bearer\ttoken",
            "Basic aaa.bbb.ccc",
            "Bearer token,token",
            "Bearer token token",
            "Bearer aaa.bbb.ccc=",
            "Bearer aaa.bbb.ccc, Bearer ddd.eee.fff",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, value.parse().unwrap());
            assert!(
                matches!(bearer_token(&headers), Err(AppError::Unauthorized)),
                "{value:?}"
            );
        }
        let mut headers = HeaderMap::new();
        headers.append(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer aaa.bbb.ccc"),
        );
        headers.append(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer ddd.eee.fff"),
        );
        assert!(matches!(
            bearer_token(&headers),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn only_exact_current_admin_role_is_authorized() {
        for role in ["user", "Admin", "superadmin", "", "admin user"] {
            let mut user = fixture_user();
            user.role = role.into();
            assert!(matches!(require_admin(user), Err(AppError::Forbidden)));
        }
        let mut user = fixture_user();
        user.role = "admin".into();
        assert!(require_admin(user).is_ok());
    }
}
