use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, ensure};
use argon2::{
    Algorithm as PasswordAlgorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    config::JwtConfig,
    domain::{
        dtos::{
            LoginRequest, RegisterRequest, TokenResponse, UserResponse, normalize_and_validate,
        },
        models::user::{NewUser, User},
    },
    errors::{AppError, AppResult},
    repositories::UserRepository,
};

const PASSWORD_MEMORY_KIB: u32 = 19 * 1024;
const PASSWORD_ITERATIONS: u32 = 2;
const PASSWORD_PARALLELISM: u32 = 1;
const MAX_TOKEN_LENGTH: usize = 4096;

// This is a precomputed Argon2id hash of a non-user dummy password, with the same
// parameters as real registrations. It is deliberately public, contains no user
// secret, and avoids doing expensive hashing in the synchronous constructor.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$YXBpLWR1bW15LXNhbHQtdjE$UT1r7qVkl4b2sTnSSsSUGTbh7MDMWPlPFLMlIaYq+CE";

#[derive(Clone, Deserialize, Serialize)]
struct Claims {
    sub: String,
    iss: String,
    aud: String,
    iat: u64,
    nbf: u64,
    exp: u64,
}

pub struct AuthService {
    repository: Arc<dyn UserRepository>,
    hash_slots: Arc<Semaphore>,
    password_hasher: Argon2<'static>,
    dummy_password_hash: String,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    validation: Validation,
    issuer: String,
    audience: String,
    ttl_secs: u64,
}

impl AuthService {
    pub fn new(
        repository: Arc<dyn UserRepository>,
        jwt: JwtConfig,
        hash_concurrency: usize,
    ) -> anyhow::Result<Self> {
        ensure!(
            jwt.secret.len() >= 32,
            "JWT secret must contain at least 32 bytes"
        );
        ensure!(
            !jwt.issuer.trim().is_empty(),
            "JWT issuer must not be empty"
        );
        ensure!(
            !jwt.audience.trim().is_empty(),
            "JWT audience must not be empty"
        );
        ensure!(jwt.ttl_secs > 0, "JWT lifetime must be positive");
        ensure!(
            (1..=Semaphore::MAX_PERMITS).contains(&hash_concurrency),
            "password hashing concurrency is outside the supported range",
        );
        let parameters = Params::new(
            PASSWORD_MEMORY_KIB,
            PASSWORD_ITERATIONS,
            PASSWORD_PARALLELISM,
            Some(32),
        )
        .map_err(|_| anyhow!("invalid password hashing configuration"))?;
        PasswordHash::new(DUMMY_PASSWORD_HASH)
            .map_err(|_| anyhow!("invalid internal dummy password hash"))?;

        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "nbf"]);
        validation.set_issuer(&[&jwt.issuer]);
        validation.set_audience(&[&jwt.audience]);
        validation.leeway = 0;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = true;

        Ok(Self {
            repository,
            hash_slots: Arc::new(Semaphore::new(hash_concurrency)),
            password_hasher: Argon2::new(PasswordAlgorithm::Argon2id, Version::V0x13, parameters),
            dummy_password_hash: DUMMY_PASSWORD_HASH.to_owned(),
            encoding_key: EncodingKey::from_secret(jwt.secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(jwt.secret.as_bytes()),
            validation,
            issuer: jwt.issuer,
            audience: jwt.audience,
            ttl_secs: jwt.ttl_secs,
        })
    }

    pub async fn register(&self, input: RegisterRequest) -> AppResult<TokenResponse> {
        let user = self.create_user_record(input).await?;
        self.issue_token(user)
    }

    /// Admin creation shares registration's validation, password policy, and
    /// transactional welcome job, but never returns a token for the new user.
    pub async fn create_user(&self, input: RegisterRequest) -> AppResult<UserResponse> {
        self.create_user_record(input).await.map(UserResponse::from)
    }

    async fn create_user_record(&self, input: RegisterRequest) -> AppResult<User> {
        let input = normalize_and_validate(input)?;
        let password_hash = self.hash_password(input.password).await?;
        self.repository
            .create_with_welcome_job(NewUser {
                user_id: Uuid::now_v7(),
                name: input.name,
                email: input.email,
                password_hash,
            })
            .await
    }

    pub async fn login(&self, input: LoginRequest) -> AppResult<TokenResponse> {
        let input = normalize_and_validate(input)?;
        let credentials = self
            .repository
            .find_credentials_by_email(&input.email)
            .await?;
        let (user, password_hash) = match credentials {
            Some(credentials) => (Some(credentials.user), credentials.password_hash),
            None => (None, self.dummy_password_hash.clone()),
        };

        // Missing accounts perform the same Argon2 work as wrong passwords. The
        // account-presence decision happens only after password verification.
        let matches = self.verify_password(input.password, password_hash).await?;
        match user {
            Some(user) if matches => self.issue_token(user),
            _ => Err(AppError::Unauthorized),
        }
    }

    pub async fn authenticate(&self, token: &str) -> AppResult<User> {
        if token.is_empty() || token.len() > MAX_TOKEN_LENGTH {
            return Err(AppError::Unauthorized);
        }
        let claims = decode::<Claims>(token, &self.decoding_key, &self.validation)
            .map_err(|_| AppError::Unauthorized)?
            .claims;
        let now = unix_timestamp()?;
        // Explicitly reject the expiration boundary, future issue times, and
        // inverted lifetimes as well as the library's signature/claim checks.
        if claims.exp <= now || claims.iat > now || claims.exp <= claims.iat {
            return Err(AppError::Unauthorized);
        }
        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| AppError::Unauthorized)?;
        if user_id.is_nil() {
            return Err(AppError::Unauthorized);
        }

        // Role is deliberately absent from JWTs. Every request reads the current
        // database record, so deletion and role changes take effect immediately.
        self.repository
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)
    }

    fn issue_token(&self, user: User) -> AppResult<TokenResponse> {
        let now = unix_timestamp()?;
        let exp = now
            .checked_add(self.ttl_secs)
            .ok_or_else(|| AppError::Internal(anyhow!("JWT expiration overflow")))?;
        let claims = Claims {
            sub: user.user_id.to_string(),
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            iat: now,
            nbf: now,
            exp,
        };
        let access_token = encode(&Header::new(Algorithm::HS256), &claims, &self.encoding_key)
            .context("failed to issue access token")?;
        Ok(TokenResponse {
            access_token,
            token_type: "Bearer",
            expires_in: self.ttl_secs,
            user: user.into(),
        })
    }

    async fn hash_password(&self, password: String) -> AppResult<String> {
        let hasher = self.password_hasher.clone();
        self.run_password_job(move || {
            let salt = SaltString::generate(&mut OsRng);
            hasher
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|_| AppError::Internal(anyhow!("password hashing failed")))
        })
        .await
    }

    async fn verify_password(&self, password: String, encoded_hash: String) -> AppResult<bool> {
        let hasher = self.password_hasher.clone();
        self.run_password_job(move || {
            let hash = PasswordHash::new(&encoded_hash)
                .map_err(|_| AppError::Internal(anyhow!("stored password hash is invalid")))?;
            if hash.algorithm != argon2::ARGON2ID_IDENT {
                return Err(AppError::Internal(anyhow!(
                    "stored password algorithm is unsupported"
                )));
            }
            match hasher.verify_password(password.as_bytes(), &hash) {
                Ok(()) => Ok(true),
                Err(argon2::password_hash::Error::Password) => Ok(false),
                Err(_) => Err(AppError::Internal(anyhow!("password verification failed"))),
            }
        })
        .await
    }

    async fn run_password_job<T, F>(&self, work: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> AppResult<T> + Send + 'static,
    {
        let permit = self
            .hash_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::Unavailable)?;
        tokio::task::spawn_blocking(move || {
            // Dropping an HTTP request does not stop spawn_blocking. Retain the
            // permit inside the task until its CPU/memory work actually ends.
            let _permit = permit;
            work()
        })
        .await
        .map_err(|_| AppError::Internal(anyhow!("password worker did not complete")))?
    }
}

fn unix_timestamp() -> AppResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|_| AppError::Internal(anyhow!("system clock is before the Unix epoch")))
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::Ordering, time::Duration};

    use super::*;
    use crate::services::test_support::{MemoryUsers, fixture_user, jwt_config};

    fn service(repository: Arc<MemoryUsers>) -> AuthService {
        AuthService::new(repository, jwt_config(), 2).unwrap()
    }

    fn claims_for(user: &User) -> Claims {
        let now = unix_timestamp().unwrap();
        Claims {
            sub: user.user_id.to_string(),
            iss: jwt_config().issuer,
            aud: jwt_config().audience,
            iat: now - 1,
            nbf: now - 1,
            exp: now + 600,
        }
    }

    fn sign(claims: &Claims) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(jwt_config().secret.as_bytes()),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn jwt_rejects_wrong_issuer_audience_expiry_subject_and_future_times() {
        let repository = Arc::new(MemoryUsers::default());
        let user = fixture_user();
        repository.insert(user.clone());
        let auth = service(repository.clone());
        let original = claims_for(&user);
        let mut invalid = Vec::new();
        let mut claims = original.clone();
        claims.iss = "other-service".into();
        invalid.push(claims);
        let mut claims = original.clone();
        claims.aud = "other-client".into();
        invalid.push(claims);
        let mut claims = original.clone();
        claims.exp = unix_timestamp().unwrap() - 1;
        invalid.push(claims);
        let mut claims = original.clone();
        claims.exp = unix_timestamp().unwrap();
        invalid.push(claims);
        let mut claims = original.clone();
        claims.sub = "not-a-uuid".into();
        invalid.push(claims);
        let mut claims = original.clone();
        claims.sub = Uuid::nil().to_string();
        invalid.push(claims);
        let mut claims = original.clone();
        claims.nbf = unix_timestamp().unwrap() + 60;
        invalid.push(claims);
        let mut claims = original.clone();
        claims.iat = unix_timestamp().unwrap() + 60;
        invalid.push(claims);
        for claims in invalid {
            assert!(matches!(
                auth.authenticate(&sign(&claims)).await,
                Err(AppError::Unauthorized)
            ));
        }
        assert_eq!(repository.id_lookups.load(Ordering::Relaxed), 0);
        assert_eq!(
            auth.authenticate(&sign(&original)).await.unwrap().user_id,
            user.user_id
        );
    }

    #[tokio::test]
    async fn jwt_rejects_wrong_signature_algorithm_and_missing_required_claims() {
        let repository = Arc::new(MemoryUsers::default());
        let user = fixture_user();
        repository.insert(user.clone());
        let auth = service(repository.clone());
        let claims = claims_for(&user);
        let wrong_signature = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"another-long-but-different-secret-value"),
        )
        .unwrap();
        let wrong_algorithm = encode(
            &Header::new(Algorithm::HS384),
            &claims,
            &EncodingKey::from_secret(jwt_config().secret.as_bytes()),
        )
        .unwrap();
        for token in [
            &wrong_signature,
            &wrong_algorithm,
            "invalid",
            "",
            &"a".repeat(4097),
        ] {
            assert!(matches!(
                auth.authenticate(token).await,
                Err(AppError::Unauthorized)
            ));
        }
        for field in ["exp", "iss", "aud", "sub", "nbf", "iat"] {
            let mut claims = serde_json::to_value(&claims).unwrap();
            claims.as_object_mut().unwrap().remove(field);
            let token = encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(jwt_config().secret.as_bytes()),
            )
            .unwrap();
            assert!(matches!(
                auth.authenticate(&token).await,
                Err(AppError::Unauthorized)
            ));
        }
        assert_eq!(repository.id_lookups.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn authentication_reads_current_role_and_rejects_deleted_users() {
        let repository = Arc::new(MemoryUsers::default());
        let user = fixture_user();
        let user_id = user.user_id;
        repository.insert(user.clone());
        let token = sign(&claims_for(&user));
        let auth = service(repository.clone());
        assert_eq!(auth.authenticate(&token).await.unwrap().role, "user");
        repository.set_role(user_id, "admin");
        assert_eq!(auth.authenticate(&token).await.unwrap().role, "admin");
        repository.set_role(user_id, "user");
        assert_eq!(auth.authenticate(&token).await.unwrap().role, "user");
        repository.delete(user_id).await.unwrap();
        assert!(matches!(
            auth.authenticate(&token).await,
            Err(AppError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn existing_legacy_uuid_subjects_keep_their_identity() {
        let repository = Arc::new(MemoryUsers::default());
        let mut user = fixture_user();
        // This models a row retained by the rename migration, not a new write.
        user.user_id = Uuid::new_v4();
        let legacy_user_id = user.user_id;
        let token = sign(&claims_for(&user));
        repository.insert(user);
        let auth = service(repository);
        assert_eq!(
            auth.authenticate(&token).await.unwrap().user_id,
            legacy_user_id
        );
    }

    #[tokio::test]
    async fn argon2id_uses_unique_salts_and_verifies_correct_and_wrong_passwords() {
        let auth = service(Arc::new(MemoryUsers::default()));
        let password = "correct horse battery staple".to_owned();
        let first = auth.hash_password(password.clone()).await.unwrap();
        let second = auth.hash_password(password.clone()).await.unwrap();
        assert!(first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert_ne!(first, second);
        assert!(
            auth.verify_password(password.clone(), first.clone())
                .await
                .unwrap()
        );
        assert!(
            !auth
                .verify_password("wrong password".into(), first)
                .await
                .unwrap()
        );
        assert!(matches!(
            auth.verify_password(password, "malformed".into()).await,
            Err(AppError::Internal(_))
        ));
        assert!(
            auth.verify_password(
                "dummy-password-never-used-by-a-user".into(),
                DUMMY_PASSWORD_HASH.into(),
            )
            .await
            .unwrap()
        );
    }

    #[tokio::test]
    async fn registration_and_admin_creation_use_normalized_atomic_user_creation() {
        let repository = Arc::new(MemoryUsers::default());
        let auth = service(repository.clone());
        let registration = auth
            .register(RegisterRequest {
                name: "  Ada  ".into(),
                email: "  ADA@EXAMPLE.COM  ".into(),
                password: "correct horse battery".into(),
            })
            .await
            .unwrap();
        assert_eq!(registration.user.name, "Ada");
        assert_eq!(registration.user.email, "ada@example.com");
        assert_eq!(registration.user.role, "user");
        assert_eq!(registration.user.user_id.get_version_num(), 7);
        assert_eq!(registration.token_type, "Bearer");
        assert_eq!(registration.expires_in, jwt_config().ttl_secs);
        assert_eq!(
            auth.authenticate(&registration.access_token)
                .await
                .unwrap()
                .user_id,
            registration.user.user_id
        );
        assert_eq!(repository.created_jobs.load(Ordering::Relaxed), 1);
        let login = auth
            .login(LoginRequest {
                email: "  ADA@EXAMPLE.COM  ".into(),
                password: "correct horse battery".into(),
            })
            .await
            .unwrap();
        assert_eq!(login.user.user_id, registration.user.user_id);
        let created = auth
            .create_user(RegisterRequest {
                name: "Grace".into(),
                email: "grace@example.com".into(),
                password: "another correct password".into(),
            })
            .await
            .unwrap();
        assert_eq!(created.role, "user");
        assert_eq!(created.user_id.get_version_num(), 7);
        assert_eq!(repository.created_jobs.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn unknown_accounts_and_wrong_passwords_have_same_public_error() {
        let repository = Arc::new(MemoryUsers::default());
        let user = fixture_user();
        let known_email = user.email.clone();
        repository.insert_with_password_hash(user, DUMMY_PASSWORD_HASH.into());
        let auth = service(repository);
        for (email, password) in [
            (known_email, "wrong password"),
            ("unknown@example.com".into(), "wrong password"),
            (
                "unknown@example.com".into(),
                "dummy-password-never-used-by-a-user",
            ),
        ] {
            assert!(matches!(
                auth.login(LoginRequest {
                    email,
                    password: password.into()
                })
                .await,
                Err(AppError::Unauthorized)
            ));
        }
    }

    #[tokio::test]
    async fn unknown_accounts_still_consume_bounded_password_capacity() {
        let auth = AuthService::new(Arc::new(MemoryUsers::default()), jwt_config(), 1).unwrap();
        let _permit = auth.hash_slots.acquire().await.unwrap();
        let result = auth
            .login(LoginRequest {
                email: "unknown@example.com".into(),
                password: "correctly shaped password".into(),
            })
            .await;
        assert!(matches!(result, Err(AppError::Unavailable)));
    }

    #[tokio::test]
    async fn cancelling_async_waiter_keeps_hash_permit_until_blocking_task_finishes() {
        let auth =
            Arc::new(AuthService::new(Arc::new(MemoryUsers::default()), jwt_config(), 1).unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_auth = auth.clone();
        let request = tokio::spawn(async move {
            worker_auth
                .run_password_job(move || {
                    let _ = started_tx.send(());
                    release_rx
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|_| AppError::Internal(anyhow!("test worker timed out")))?;
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(auth.hash_slots.available_permits(), 0);
        assert!(matches!(
            auth.run_password_job(|| Ok(())).await,
            Err(AppError::Unavailable)
        ));
        release_tx.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(5), auth.hash_slots.acquire())
            .await
            .expect("blocking task must eventually release its permit")
            .unwrap();
        drop(permit);
        assert_eq!(auth.hash_slots.available_permits(), 1);
    }
}
