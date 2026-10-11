//! Test-only adapter over the production RPC scheduler. No synthetic handlers or larger limits.
use super::*;
use bounded_dispatch::{
    rpc_has_global_mutation_fence, rpc_ordering_key, RpcDispatchClass, RpcOwnerResolver,
};

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
        let owner = rpc_ordering_key(&request);
        // Keep the same authoritative alias resolution as run_request_loop. The benchmark
        // intentionally retains the current global write fence; bypassing it hides the defect.
        let resolver: Option<RpcOwnerResolver> = if matches!(
            class,
            RpcDispatchClass::Control | RpcDispatchClass::Approval
        ) {
            request
                .params
                .as_ref()
                .and_then(|p| p["runId"].as_str())
                .map(|run| {
                    let run = run.to_owned();
                    let owner_service = service.clone();
                    Box::new(move || {
                        owner_service
                            .conversation_id_for_run(&run)
                            .map(|conversation| format!("conversation:{conversation}"))
                    }) as RpcOwnerResolver
                })
        } else {
            None
        };
        let global_fence = rpc_has_global_mutation_fence(&request);
        self.0.try_submit_ordered(
            class,
            request.id.clone(),
            owner,
            resolver,
            global_fence,
            move || {
                began();
                handle_request(&storage, &service, outbound, request)
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
