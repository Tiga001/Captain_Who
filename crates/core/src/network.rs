//! Desktop-owned proxy selection. Route decisions are refreshed for each request, never persisted
//! in conversation state. Standalone Core consumers retain reqwest's system proxy behavior.

use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NetworkRoute {
    Direct,
    HttpProxy { host: String, port: u16 },
    HttpsProxy { host: String, port: u16 },
    Socks5Proxy { host: String, port: u16 },
    Socks4Proxy { host: String, port: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkRouteError;

impl fmt::Display for NetworkRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("System network route could not be resolved")
    }
}

impl std::error::Error for NetworkRouteError {}

/// Runs on blocking workers (including the existing skills dispatcher), never on a Tokio worker.
pub trait NetworkRouteResolver: Send + Sync {
    fn resolve(&self, url: &str) -> Result<NetworkRoute, NetworkRouteError>;
}

static HOST_RESOLVER: OnceLock<Arc<dyn NetworkRouteResolver>> = OnceLock::new();

pub fn has_host_route_resolver() -> bool {
    HOST_RESOLVER.get().is_some()
}

pub fn validate_request_url(value: &str) -> Result<reqwest::Url, NetworkRouteError> {
    let url = reqwest::Url::parse(value).map_err(|_| NetworkRouteError)?;
    if value.len() > 32 * 1024
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
    {
        return Err(NetworkRouteError);
    }
    Ok(url)
}

fn is_loopback_url(value: &str) -> Result<bool, NetworkRouteError> {
    let url = validate_request_url(value)?;
    Ok(match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
        None => false,
    })
}

pub fn install_host_route_resolver(
    resolver: Arc<dyn NetworkRouteResolver>,
) -> Result<(), NetworkRouteError> {
    HOST_RESOLVER.set(resolver).map_err(|_| NetworkRouteError)
}

impl NetworkRoute {
    pub fn validate(&self) -> Result<(), NetworkRouteError> {
        match self {
            Self::Direct => Ok(()),
            Self::HttpProxy { host, port }
            | Self::HttpsProxy { host, port }
            | Self::Socks5Proxy { host, port }
            | Self::Socks4Proxy { host, port } => {
                if *port == 0 || host.is_empty() || host.len() > 253 {
                    return Err(NetworkRouteError);
                }
                // Host alone: no credentials, paths, query, or URL syntax may enter a proxy URL.
                if host.parse::<std::net::IpAddr>().is_err()
                    && !host.split('.').all(|label| {
                        !label.is_empty()
                            && label.len() <= 63
                            && label
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                            && !label.starts_with('-')
                            && !label.ends_with('-')
                    })
                {
                    return Err(NetworkRouteError);
                }
                Ok(())
            }
        }
    }

    fn proxy(&self) -> Result<Option<reqwest::Proxy>, NetworkRouteError> {
        self.validate()?;
        let (scheme, host, port) = match self {
            Self::Direct => return Ok(None),
            Self::HttpProxy { host, port } => ("http", host, port),
            Self::HttpsProxy { host, port } => ("https", host, port),
            Self::Socks5Proxy { host, port } => ("socks5h", host, port),
            Self::Socks4Proxy { host, port } => ("socks4a", host, port),
        };
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        reqwest::Proxy::all(format!("{scheme}://{host}:{port}"))
            .map(Some)
            .map_err(|_| NetworkRouteError)
    }
}

async fn host_route(url: &str) -> Result<Option<NetworkRoute>, NetworkRouteError> {
    if is_loopback_url(url)? {
        return Ok(Some(NetworkRoute::Direct));
    }
    let Some(resolver) = HOST_RESOLVER.get().cloned() else {
        return Ok(None);
    };
    let url = url.to_string();
    let route = tokio::task::spawn_blocking(move || resolver.resolve(&url))
        .await
        .map_err(|_| NetworkRouteError)??;
    route.validate()?;
    Ok(Some(route))
}

/// The artifact downloader has always required explicit direct IP pinning outside the desktop.
/// A configured Host failure is an error, never an implicit direct fallback.
pub async fn resolve_route(url: &reqwest::Url) -> Result<NetworkRoute, NetworkRouteError> {
    Ok(host_route(url.as_str())
        .await?
        .unwrap_or(NetworkRoute::Direct))
}

pub async fn client_builder_for_url(
    url: &str,
) -> Result<reqwest::ClientBuilder, NetworkRouteError> {
    let builder = reqwest::Client::builder();
    match host_route(url).await? {
        None => Ok(builder),
        Some(route) => match route.proxy()? {
            Some(proxy) => Ok(builder.no_proxy().proxy(proxy)),
            None => Ok(builder.no_proxy()),
        },
    }
}

pub fn blocking_client_builder_for_url(
    url: &str,
) -> Result<reqwest::blocking::ClientBuilder, NetworkRouteError> {
    let builder = reqwest::blocking::Client::builder();
    if is_loopback_url(url)? {
        return Ok(builder.no_proxy());
    }
    let Some(resolver) = HOST_RESOLVER.get() else {
        return Ok(builder);
    };
    match resolver.resolve(url)?.proxy()? {
        Some(proxy) => Ok(builder.no_proxy().proxy(proxy)),
        None => Ok(builder.no_proxy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_contract_accepts_ipv6_and_rejects_url_or_credential_injection() {
        for host in ["127.0.0.1", "::1", "proxy.example.test"] {
            let route = NetworkRoute::HttpProxy {
                host: host.into(),
                port: 7897,
            };
            assert!(route.proxy().is_ok());
            assert_eq!(
                serde_json::from_value::<NetworkRoute>(serde_json::to_value(&route).unwrap())
                    .unwrap(),
                route
            );
        }
        for host in [
            "user:secret@proxy.test",
            "proxy.test/path",
            "proxy.test?x=y",
            "",
            "proxy\r\n.test",
        ] {
            assert!(NetworkRoute::HttpProxy {
                host: host.into(),
                port: 7897
            }
            .proxy()
            .is_err());
        }
        assert!(NetworkRoute::Socks5Proxy {
            host: "localhost".into(),
            port: 0
        }
        .validate()
        .is_err());
    }

    #[test]
    fn all_supported_routes_build_without_exposing_authentication() {
        let variants = [
            NetworkRoute::Direct,
            NetworkRoute::HttpProxy {
                host: "localhost".into(),
                port: 7897,
            },
            NetworkRoute::HttpsProxy {
                host: "localhost".into(),
                port: 7897,
            },
            NetworkRoute::Socks5Proxy {
                host: "localhost".into(),
                port: 7897,
            },
            NetworkRoute::Socks4Proxy {
                host: "localhost".into(),
                port: 7897,
            },
        ];
        for route in variants {
            assert!(route.proxy().is_ok(), "{route:?}");
        }
    }
}
