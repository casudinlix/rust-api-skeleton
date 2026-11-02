//! Run explicitly against a disposable Redis with TEST_REDIS_URL. Every test
//! owns a unique prefix and removes only its keys; no global flush is used.

mod support;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rust_api_skeleton::{
    cache::{Cache, CacheError},
    config::cache::{CacheConfig, CacheDriver},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn settings() -> CacheConfig {
    CacheConfig {
        driver: CacheDriver::Redis,
        redis_url: support::redis_url().expect("load test Redis URL"),
        prefix: format!("rust-api-test:{}:cache:", Uuid::new_v4()),
        default_ttl_secs: 10,
        timeout_ms: 1000,
        ..CacheConfig::default()
    }
}

async fn raw_connection(config: &CacheConfig) -> redis::aio::MultiplexedConnection {
    redis::Client::open(config.redis_url.as_str())
        .expect("invalid test Redis connection settings")
        .get_multiplexed_async_connection()
        .await
        .expect("test Redis is unavailable")
}

async fn set_raw(config: &CacheConfig, key: &str, value: &[u8]) {
    let mut connection = raw_connection(config).await;
    redis::cmd("SET")
        .arg(format!("{}{key}", config.prefix))
        .arg(value)
        .arg("EX")
        .arg(30)
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
async fn redis_cache_round_trip_expiry_and_namespaces() {
    let config = settings();
    let cache = Cache::from_config(&config).unwrap();
    cache.ping().await.unwrap();
    assert_eq!(cache.get::<serde_json::Value>("entry").await.unwrap(), None);
    let expected = json!({"count": 12, "items": [1, 2, 3]});
    cache.put("entry", &expected, None).await.unwrap();
    assert_eq!(
        cache
            .clone()
            .get::<serde_json::Value>("entry")
            .await
            .unwrap(),
        Some(expected)
    );
    let mut raw = raw_connection(&config).await;
    let ttl: i64 = redis::cmd("PTTL")
        .arg(format!("{}entry", config.prefix))
        .query_async(&mut raw)
        .await
        .unwrap();
    assert!(ttl > 0 && ttl <= 10_000, "default TTL was not applied");

    cache
        .put("entry", &42_u32, Some(Duration::from_millis(500)))
        .await
        .unwrap();
    assert_eq!(cache.get::<u32>("entry").await.unwrap(), Some(42));
    let ttl: i64 = redis::cmd("PTTL")
        .arg(format!("{}entry", config.prefix))
        .query_async(&mut raw)
        .await
        .unwrap();
    assert!(ttl > 0 && ttl <= 500, "overwrite did not replace the TTL");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(cache.get::<u32>("entry").await.unwrap(), None);

    cache.put("null", &Option::<u32>::None, None).await.unwrap();
    assert_eq!(cache.get::<Option<u32>>("null").await.unwrap(), Some(None));
    assert!(cache.forget("null").await.unwrap());
    assert!(!cache.forget("null").await.unwrap());

    let other = Cache::from_config(&CacheConfig {
        prefix: format!("{}other:", config.prefix),
        ..config
    })
    .unwrap();
    cache.put("shared", &1_u32, None).await.unwrap();
    assert_eq!(other.get::<u32>("shared").await.unwrap(), None);
    other.put("shared", &2_u32, None).await.unwrap();
    assert_eq!(cache.get::<u32>("shared").await.unwrap(), Some(1));
    assert_eq!(other.get::<u32>("shared").await.unwrap(), Some(2));
    assert!(cache.forget("shared").await.unwrap());
    assert!(other.forget("shared").await.unwrap());
}

#[tokio::test]
#[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
async fn redis_cache_rejects_corrupt_and_oversized_values() {
    let config = CacheConfig {
        max_value_bytes: 64,
        ..settings()
    };
    let cache = Cache::from_config(&config).unwrap();
    cache.put("value", &"x".repeat(62), None).await.unwrap();
    assert_eq!(
        cache.put("value", &"x".repeat(63), None).await,
        Err(CacheError::ValueTooLarge)
    );
    assert_eq!(
        cache.get::<String>("value").await.unwrap(),
        Some("x".repeat(62))
    );

    set_raw(&config, "corrupt", b"private-secret-value-not-json").await;
    let error = cache.get::<u32>("corrupt").await.unwrap_err();
    assert_eq!(error, CacheError::Serialization);
    assert!(!format!("{error:?}: {error}").contains("secret-value"));
    set_raw(&config, "oversized", &vec![b'x'; 16_384]).await;
    assert_eq!(
        cache.get::<String>("oversized").await,
        Err(CacheError::ValueTooLarge)
    );

    let recovered = cache
        .remember("corrupt", None, || async { Ok::<_, ()>(55_u32) })
        .await;
    assert_eq!(recovered, Ok(55));
    assert_eq!(cache.get::<u32>("corrupt").await.unwrap(), Some(55));
    for key in ["value", "corrupt", "oversized"] {
        cache.forget(key).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
async fn redis_remember_serves_hits_and_does_not_cache_loader_failures() {
    let config = CacheConfig {
        max_value_bytes: 64,
        ..settings()
    };
    let cache = Cache::from_config(&config).unwrap();
    let loads = AtomicUsize::new(0);
    for _ in 0..2 {
        assert_eq!(
            cache
                .remember("loaded", None, || async {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(77_u32)
                })
                .await,
            Ok(77)
        );
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(
        cache
            .remember::<u32, _, _, _>("failed", None, || async { Err("loader failed") })
            .await,
        Err("loader failed")
    );
    assert_eq!(cache.get::<u32>("failed").await.unwrap(), None);

    let loaded = "x".repeat(100);
    assert_eq!(
        cache
            .remember("large", None, || async { Ok::<_, ()>(loaded.clone()) })
            .await,
        Ok(loaded)
    );
    assert_eq!(cache.get::<String>("large").await.unwrap(), None);
    cache.forget("loaded").await.unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
async fn redis_authentication_failure_is_bounded_and_remember_falls_back() {
    let mut config = settings();
    let mut url = url::Url::parse(&config.redis_url).unwrap();
    url.set_username("nonexistent-cache-test-user").unwrap();
    url.set_password(Some("secret-value-must-never-leak"))
        .unwrap();
    config.redis_url = url.to_string();
    config.timeout_ms = 100;
    let cache = Cache::from_config(&config).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), cache.ping())
        .await
        .expect("cache authentication failure exceeded its deadline")
        .unwrap_err();
    assert!(matches!(
        error,
        CacheError::Unavailable | CacheError::Timeout
    ));
    let safe = format!("{error:?}: {error}");
    assert!(!safe.contains("secret-value"));
    assert!(!safe.contains("nonexistent-cache-test-user"));
    assert!(!safe.contains("127.0.0.1"));
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        cache.remember("fallback", None, || async { Ok::<_, ()>(88_u32) }),
    )
    .await
    .expect("cache fallback exceeded read and write deadlines");
    assert_eq!(result, Ok(88));
}

#[tokio::test]
#[ignore = "requires disposable Redis admin access to create and remove one isolated ACL user"]
async fn redis_recovers_when_initial_authentication_configuration_is_fixed() {
    let mut config = settings();
    let mut admin = raw_connection(&config).await;
    let username = format!("cache-test-{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    let mut url = url::Url::parse(&config.redis_url).unwrap();
    url.set_username(&username).unwrap();
    url.set_password(Some(&password)).unwrap();
    config.redis_url = url.to_string();
    let cache = Cache::from_config(&config).unwrap();
    assert_eq!(cache.ping().await, Err(CacheError::Unavailable));

    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&username)
        .arg("reset")
        .arg("on")
        .arg(format!(">{password}"))
        .arg(format!("~{}*", config.prefix))
        .arg("+@connection")
        .arg("+eval")
        .arg("+strlen")
        .arg("+get")
        .arg("+set")
        .arg("+del")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    let recovered = cache.ping().await;
    // Remove only the test-owned ACL user, even if the recovery assertion fails.
    let removed: u64 = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&username)
        .query_async(&mut admin)
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(
        recovered,
        Ok(()),
        "a failed initial connection remained cached after Redis credentials became valid"
    );
}

#[tokio::test]
#[ignore = "uses loopback sockets for unavailable and unresponsive Redis checks"]
async fn redis_connection_failure_and_silent_server_have_bounded_deadlines() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let config = CacheConfig {
        driver: CacheDriver::Redis,
        redis_url: format!("redis://:secret-value@{address}/0"),
        timeout_ms: 50,
        ..CacheConfig::default()
    };
    let cache = Cache::from_config(&config).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), cache.ping())
        .await
        .expect("unavailable Redis exceeded its deadline")
        .unwrap_err();
    assert!(matches!(
        error,
        CacheError::Unavailable | CacheError::Timeout
    ));
    assert!(!format!("{error:?}: {error}").contains("secret-value"));
    assert_eq!(
        cache
            .remember("key", None, || async { Ok::<_, ()>(91_u32) })
            .await,
        Ok(91)
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = CacheConfig {
        redis_url: format!("redis://{}/0", listener.local_addr().unwrap()),
        ..config
    };
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let cache = Cache::from_config(&config).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), cache.ping())
            .await
            .expect("unresponsive Redis exceeded its deadline"),
        Err(CacheError::Timeout)
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
#[ignore = "requires a disposable plaintext redis:// TEST_REDIS_URL and loopback sockets"]
async fn redis_reconnects_after_a_transport_disconnect() {
    let mut config = settings();
    let mut url = url::Url::parse(&config.redis_url).unwrap();
    assert_eq!(
        url.scheme(),
        "redis",
        "fault proxy test requires a plaintext redis:// test endpoint"
    );
    let upstream = (
        url.host_str().unwrap().to_owned(),
        url.port().unwrap_or(6379),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    url.set_host(Some("127.0.0.1")).unwrap();
    url.set_port(Some(proxy_address.port())).unwrap();
    config.redis_url = url.to_string();

    let stop = CancellationToken::new();
    let stopping = stop.clone();
    let cut = CancellationToken::new();
    let cutting = cut.clone();
    let (cut_done, cut_observed) = tokio::sync::oneshot::channel();
    let connections = Arc::new(AtomicUsize::new(0));
    let accepted = connections.clone();
    let proxy = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        let mut cut_done = Some(cut_done);
        loop {
            tokio::select! {
                _ = stopping.cancelled() => break,
                incoming = listener.accept() => {
                    let (mut incoming, _) = incoming.unwrap();
                    let first = accepted.fetch_add(1, Ordering::SeqCst) == 0;
                    let disconnect = if first { cutting.clone() } else { CancellationToken::new() };
                    let acknowledged = if first { cut_done.take() } else { None };
                    let target = upstream.clone();
                    let stop_connection = stopping.clone();
                    tasks.spawn(async move {
                        let mut outgoing = tokio::net::TcpStream::connect(target).await.unwrap();
                        tokio::select! {
                            _ = stop_connection.cancelled() => {},
                            _ = disconnect.cancelled() => {},
                            _ = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing) => {},
                        }
                        drop(incoming);
                        drop(outgoing);
                        if let Some(acknowledged) = acknowledged { let _ = acknowledged.send(()); }
                    });
                }
            }
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    });

    let cache = Cache::from_config(&config).unwrap();
    cache.put("reconnect", &73_u32, None).await.unwrap();
    cut.cancel();
    tokio::time::timeout(Duration::from_secs(1), cut_observed)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match cache.get::<u32>("reconnect").await {
                Ok(Some(73)) => break,
                Err(CacheError::Unavailable | CacheError::Timeout) => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                other => panic!("unexpected result after reconnect: {other:?}"),
            }
        }
    })
    .await
    .expect("cache did not recover after the transport connection was cut");
    assert!(
        connections.load(Ordering::SeqCst) >= 2,
        "cache did not establish a replacement connection"
    );
    assert!(cache.forget("reconnect").await.unwrap());
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(1), proxy)
        .await
        .unwrap()
        .unwrap();
}
