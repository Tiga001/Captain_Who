//! Test-only adapter over the production RPC scheduler. No synthetic handlers or larger limits.
use super::*;
use bounded_dispatch::rpc_scope_resolver;

pub(crate) struct BaselineDispatcher(RpcRequestDispatcher);

impl BaselineDispatcher {
    pub(crate) fn new(outbound: OutboundSender) -> Self {
        Self(RpcRequestDispatcher::new(outbound))
    }

    pub(crate) fn submit(
        &self,
        storage: Arc<StorageService>,
        service: AgentService,
        outbound: OutboundSender,
        request: JsonRpcRequest,
        began: impl FnOnce() + Send + 'static,
    ) -> Result<(), Value> {
        let class = rpc_dispatch_class(&request);
        let resolver = rpc_scope_resolver(service.clone(), &request);
        let queued_access = bounded_dispatch::rpc_requires_queued_access(&request.method)
            .then(|| service.capture_execution_access());
        let retained_bytes = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": &request.id,
            "method": &request.method, "params": &request.params,
        }))
        .map_or(usize::MAX, |bytes| bytes.len())
        .saturating_add(512);
        self.0.try_submit_scoped(
            class,
            request.id.clone(),
            retained_bytes,
            resolver,
            move || {
                began();
                let handle = || handle_request(&storage, &service, outbound, request);
                if let Some(token) = queued_access {
                    service.with_queued_execution_access(token, handle)
                } else {
                    handle()
                }
            },
        )
    }

    pub(crate) async fn shutdown(self) {
        self.0
            .shutdown()
            .await
            .expect("baseline RPC scheduler drains");
    }
}
