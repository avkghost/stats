use anyhow::{Context, Result};
use hickory_resolver::TokioResolver;
use stats_core::AccessEvent;
use std::{collections::HashMap, net::IpAddr, time::Duration};
use tokio::{runtime::Handle, sync::Semaphore, task::JoinSet, time::timeout};

pub struct ReverseDnsEnricher {
    resolver: TokioResolver,
    cache: HashMap<IpAddr, Option<String>>,
    semaphore: std::sync::Arc<Semaphore>,
    timeout: Duration,
    cache_size: usize,
}

impl ReverseDnsEnricher {
    pub fn new(timeout_ms: u64, cache_size: usize) -> Result<Self> {
        let resolver = TokioResolver::builder_tokio()
            .context("initialize system DNS resolver")?
            .build()?;
        Ok(Self {
            resolver,
            cache: HashMap::new(),
            semaphore: std::sync::Arc::new(Semaphore::new(32)),
            timeout: Duration::from_millis(timeout_ms.max(1)),
            cache_size: cache_size.max(1),
        })
    }

    pub fn enrich_batch(&mut self, events: &mut [AccessEvent]) -> Result<()> {
        let mut pending = HashMap::<IpAddr, Vec<usize>>::new();
        for (index, event) in events.iter_mut().enumerate() {
            let Ok(ip) = event.client_ip.parse::<IpAddr>() else {
                continue;
            };
            if !is_public_address(ip) {
                continue;
            }
            if let Some(hostname) = self.cache.get(&ip) {
                event.client_hostname.clone_from(hostname);
            } else {
                pending.entry(ip).or_default().push(index);
            }
        }
        if pending.is_empty() {
            return Ok(());
        }

        let resolver = self.resolver.clone();
        let semaphore = self.semaphore.clone();
        let timeout_duration = self.timeout;
        let handle = Handle::try_current().context("reverse DNS requires a Tokio runtime")?;
        let addresses = pending.keys().copied().collect::<Vec<_>>();
        let resolved = tokio::task::block_in_place(|| {
            handle.block_on(async move {
                let mut tasks = JoinSet::new();
                for ip in addresses {
                    let resolver = resolver.clone();
                    let semaphore = semaphore.clone();
                    tasks.spawn(async move {
                        let hostname = async {
                            let _permit = semaphore.acquire_owned().await.ok()?;
                            let lookup = timeout(timeout_duration, resolver.reverse_lookup(ip))
                                .await
                                .ok()
                                .and_then(Result::ok)?;
                            lookup.answers().first().map(|record| {
                                record.data.to_string().trim_end_matches('.').to_owned()
                            })
                        }
                        .await;
                        (ip, hostname)
                    });
                }
                let mut resolved = HashMap::new();
                while let Some(result) = tasks.join_next().await {
                    if let Ok((ip, hostname)) = result {
                        resolved.insert(ip, hostname);
                    }
                }
                resolved
            })
        });

        for (ip, hostname) in resolved {
            if self.cache.len() >= self.cache_size {
                if let Some(key) = self.cache.keys().next().copied() {
                    self.cache.remove(&key);
                }
            }
            self.cache.insert(ip, hostname.clone());
            if let Some(indices) = pending.get(&ip) {
                for &index in indices {
                    events[index].client_hostname.clone_from(&hostname);
                }
            }
        }
        Ok(())
    }
}

fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
        }
        IpAddr::V6(ip) => {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_public_address;
    use std::net::IpAddr;

    #[test]
    fn skips_non_public_addresses() {
        for address in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "::1", "fc00::1"] {
            assert!(!is_public_address(address.parse::<IpAddr>().unwrap()));
        }
    }

    #[test]
    fn accepts_public_addresses() {
        assert!(is_public_address("203.0.113.10".parse::<IpAddr>().unwrap()));
    }
}
