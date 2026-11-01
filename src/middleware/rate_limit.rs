use crate::{
    errors::{AppError, AppResult},
    state::AppState,
};
use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const MAX_PEERS: usize = 10_000;

struct Window {
    start: Instant,
    count: u32,
}

/// Per-process abuse protection. Uses the actual peer address, never untrusted
/// X-Forwarded-For. Configure the reverse proxy's distributed limiter separately.
pub struct AuthRateLimiter {
    limit: u32,
    peers: Mutex<HashMap<IpAddr, Window>>,
}

impl AuthRateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            peers: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, ip: IpAddr) -> AppResult<()> {
        self.check_at(ip, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> AppResult<()> {
        let mut peers = self.peers.lock().map_err(|_| AppError::Unavailable)?;
        if peers.len() >= MAX_PEERS && !peers.contains_key(&ip) {
            peers.retain(|_, window| now.duration_since(window.start) < WINDOW);
            if peers.len() >= MAX_PEERS {
                return Err(AppError::TooManyRequests);
            }
        }
        let window = peers.entry(ip).or_insert(Window {
            start: now,
            count: 0,
        });
        if now.duration_since(window.start) >= WINDOW {
            *window = Window {
                start: now,
                count: 0,
            };
        }
        if window.count >= self.limit {
            return Err(AppError::TooManyRequests);
        }
        window.count += 1;
        Ok(())
    }
}

pub async fn auth_rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    if let Err(error) = state.auth_rate_limiter.check(ip) {
        return error.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_per_peer_and_resets_without_sleep() {
        let limiter = AuthRateLimiter::new(1);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Instant::now();
        assert!(limiter.check_at(ip, now).is_ok());
        assert!(matches!(
            limiter.check_at(ip, now),
            Err(AppError::TooManyRequests)
        ));
        assert!(
            limiter
                .check_at(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), now)
                .is_ok()
        );
        assert!(limiter.check_at(ip, now + WINDOW).is_ok());
    }
}
