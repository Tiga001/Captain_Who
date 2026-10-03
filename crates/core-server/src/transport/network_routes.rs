//! Private Host/Core reverse bridge. This carries routing metadata only, never model-visible
//! output. Completions run on the control plane so a full RPC/skills lane cannot deadlock routing.

use super::OutboundSender;
use mycopilot_core::network::{NetworkRoute, NetworkRouteError, NetworkRouteResolver};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub(super) const COMPLETE_METHOD: &str = "host.networkRoute.complete";
const RESOLVE_METHOD: &str = "host.networkRoute.resolve";
const MAX_PENDING: usize = 128;
const ROUTE_TIMEOUT: Duration = Duration::from_secs(12);

type RouteResult = Result<NetworkRoute, NetworkRouteError>;

struct State {
    outbound: Option<OutboundSender>,
    pending: HashMap<Uuid, mpsc::SyncSender<RouteResult>>,
}

pub(crate) struct NetworkRouteBridge {
    state: Mutex<State>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Completion {
    request_id: Uuid,
    route: Option<NetworkRoute>,
}

impl NetworkRouteBridge {
    fn new(outbound: OutboundSender) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                outbound: Some(outbound),
                pending: HashMap::new(),
            }),
        })
    }

    pub(super) fn install(outbound: &OutboundSender) -> std::io::Result<Option<NetworkRouteGuard>> {
        if std::env::var("CAPTAIN_HOST_NETWORK_ROUTES").as_deref() != Ok("1") {
            return Ok(None);
        }
        let bridge = Self::new(outbound.clone());
        mycopilot_core::network::install_host_route_resolver(bridge.clone())
            .map_err(std::io::Error::other)?;
        Ok(Some(NetworkRouteGuard(bridge)))
    }

    pub(super) fn complete(&self, params: Option<Value>) -> bool {
        let Some(completion) =
            params.and_then(|value| serde_json::from_value::<Completion>(value).ok())
        else {
            return false;
        };
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let Some(sender) = state.pending.remove(&completion.request_id) else {
            return false;
        };
        let result = completion.route.ok_or(NetworkRouteError).and_then(|route| {
            route.validate()?;
            Ok(route)
        });
        sender.send(result).is_ok()
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.outbound.take();
            for (_, sender) in state.pending.drain() {
                let _ = sender.send(Err(NetworkRouteError));
            }
        }
    }

    fn resolve_with_timeout(&self, url: &str, timeout: Duration) -> RouteResult {
        mycopilot_core::network::validate_request_url(url)?;
        let id = Uuid::new_v4();
        let (sender, receiver) = mpsc::sync_channel(1);
        {
            let mut state = self.state.lock().map_err(|_| NetworkRouteError)?;
            if state.pending.len() >= MAX_PENDING {
                return Err(NetworkRouteError);
            }
            let outbound = state.outbound.as_ref().ok_or(NetworkRouteError)?.clone();
            state.pending.insert(id, sender);
            if outbound
                .send(json!({"jsonrpc":"2.0", "method":RESOLVE_METHOD,
                "params":{"requestId":id,"url":url}}))
                .is_err()
            {
                state.pending.remove(&id);
                return Err(NetworkRouteError);
            }
        }
        let result = receiver
            .recv_timeout(timeout)
            .map_err(|_| NetworkRouteError)
            .and_then(|value| value);
        if let Ok(mut state) = self.state.lock() {
            state.pending.remove(&id);
        }
        result
    }
}

impl NetworkRouteResolver for NetworkRouteBridge {
    fn resolve(&self, url: &str) -> RouteResult {
        self.resolve_with_timeout(url, ROUTE_TIMEOUT)
    }
}

pub(super) struct NetworkRouteGuard(pub(super) Arc<NetworkRouteBridge>);

impl Drop for NetworkRouteGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn next_request(receiver: &mut super::super::OutboundReceiver) -> Value {
        receiver.recv().await.unwrap()
    }

    #[tokio::test]
    async fn refreshes_route_on_every_request_and_does_not_reuse_stale_proxy() {
        let (sender, mut receiver) = super::super::outbound_channel();
        let bridge = NetworkRouteBridge::new(sender);
        for route in [
            NetworkRoute::HttpProxy {
                host: "127.0.0.1".into(),
                port: 7897,
            },
            NetworkRoute::Direct,
        ] {
            let resolver = bridge.clone();
            let pending =
                tokio::task::spawn_blocking(move || resolver.resolve("https://example.test/image"));
            let request = next_request(&mut receiver).await;
            assert_eq!(request["method"], RESOLVE_METHOD);
            assert!(bridge.complete(Some(
                json!({"requestId": request["params"]["requestId"], "route": route})
            )));
            assert_eq!(pending.await.unwrap().unwrap(), route);
        }
        assert!(bridge.state.lock().unwrap().pending.is_empty());
    }

    #[tokio::test]
    async fn error_and_shutdown_wake_requests_without_falling_back_to_direct() {
        let (sender, mut receiver) = super::super::outbound_channel();
        let bridge = NetworkRouteBridge::new(sender);
        let resolver = bridge.clone();
        let pending = tokio::task::spawn_blocking(move || resolver.resolve("https://example.test"));
        let request = next_request(&mut receiver).await;
        assert!(bridge.complete(Some(
            json!({"requestId":request["params"]["requestId"],"route":null})
        )));
        assert!(pending.await.unwrap().is_err());
        let resolver = bridge.clone();
        let pending = tokio::task::spawn_blocking(move || resolver.resolve("https://example.test"));
        next_request(&mut receiver).await;
        bridge.close();
        assert!(pending.await.unwrap().is_err());
        assert!(bridge.resolve("https://example.test").is_err());
    }

    #[test]
    fn timeout_removes_pending_request_and_late_completion_is_ignored() {
        let (sender, mut receiver) = super::super::outbound_channel();
        let bridge = NetworkRouteBridge::new(sender);
        assert!(bridge
            .resolve_with_timeout("https://example.test", Duration::ZERO)
            .is_err());
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        let request = receiver.try_recv().unwrap();
        assert!(!bridge.complete(Some(
            json!({"requestId":request["params"]["requestId"],"route":{"kind":"direct"}})
        )));
    }
}
