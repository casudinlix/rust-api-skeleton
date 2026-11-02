use std::net::IpAddr;

use anyhow::ensure;

#[derive(Clone, Debug)]
pub struct ObservabilityConfig {
    pub host: IpAddr,
    pub api_port: u16,
    pub worker_port: u16,
    pub scheduler_port: u16,
    pub relay_port: u16,
}

impl ObservabilityConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut ports = [
            self.api_port,
            self.worker_port,
            self.scheduler_port,
            self.relay_port,
        ];
        ensure!(
            ports.iter().all(|port| *port > 0),
            "observability ports must be nonzero"
        );
        ports.sort_unstable();
        ensure!(
            ports.windows(2).all(|pair| pair[0] != pair[1]),
            "API/worker/scheduler/relay observability ports must be distinct"
        );
        Ok(())
    }
}
