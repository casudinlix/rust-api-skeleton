//! Container health probe without a shell, curl, or third-party dependencies.
//! This binary only checks the local process /ready endpoint over HTTP/1.1.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    process::ExitCode,
    time::Duration,
};

const HTTP_OK_11: &[u8; 13] = b"HTTP/1.1 200 ";
const HTTP_OK_10: &[u8; 13] = b"HTTP/1.0 200 ";

fn parse_port(input: &str) -> Option<u16> {
    input.parse::<u16>().ok().filter(|port| *port != 0)
}

fn is_ready_status(prefix: &[u8; 13]) -> bool {
    prefix == HTTP_OK_11 || prefix == HTTP_OK_10
}

fn check_readiness(port: u16) -> std::io::Result<bool> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let timeout = Duration::from_secs(1);
    let mut connection = TcpStream::connect_timeout(&address, timeout)?;
    connection.set_read_timeout(Some(timeout))?;
    connection.set_write_timeout(Some(timeout))?;
    connection.write_all(b"GET /ready HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;

    // We need only the status line prefix; never collect or log response bodies.
    let mut prefix = [0_u8; 13];
    connection.read_exact(&mut prefix)?;
    Ok(is_ready_status(&prefix))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(argument) = args.next() else {
        eprintln!("usage: healthcheck <port>");
        return ExitCode::FAILURE;
    };
    if argument == "--help" && args.next().is_none() {
        println!("usage: healthcheck <port>  (local HTTP /ready probe)");
        return ExitCode::SUCCESS;
    }
    if args.next().is_some() {
        eprintln!("usage: healthcheck <port>");
        return ExitCode::FAILURE;
    }
    let Some(port) = parse_port(&argument) else {
        eprintln!("healthcheck requires a nonzero TCP port");
        return ExitCode::FAILURE;
    };
    match check_readiness(port) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) | Err(_) => {
            eprintln!("readiness check failed");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_accepts_only_successful_http_status() {
        assert!(is_ready_status(b"HTTP/1.1 200 "));
        assert!(is_ready_status(b"HTTP/1.0 200 "));
        for status in [
            b"HTTP/1.1 500 ",
            b"HTTP/1.1 503 ",
            b"HTTP/1.1 204 ",
            b"HTTP/1.1 2000",
            b"HTTP/1.0 301 ",
        ] {
            assert!(!is_ready_status(status));
        }
    }

    #[test]
    fn probe_port_must_be_valid_and_nonzero() {
        assert_eq!(parse_port("9100"), Some(9100));
        for invalid in ["", "0", "-1", "65536", "not-a-port"] {
            assert_eq!(parse_port(invalid), None);
        }
    }
}
