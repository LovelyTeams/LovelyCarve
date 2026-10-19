//! Incoming LPP message routing and pending-call cancellation.
use super::host_requests::{dispatch_host_request, plugin_error_to_rpc};
use super::*;

pub(super) async fn route_message(message: LppMessage, context: &RouterContext) {
    match message {
        LppMessage::Response {
            v,
            id,
            result,
            error,
        } if v == PROTOCOL_VERSION && id % 2 == 0 => {
            let Some(response_tx) = context.pending.lock().remove(&id) else {
                tracing::debug!(
                    request_id = id,
                    "received a response for a plugin request nobody waits for"
                );
                return;
            };
            let response = match error {
                Some(error) => Err(PluginError::Rpc {
                    code: error.code,
                    message: error.message,
                }),
                None => Ok(result.unwrap_or(Value::Nil)),
            };
            if response_tx.send(response).is_err() {
                tracing::debug!(request_id = id, "plugin response receiver was dropped");
            }
        }
        LppMessage::Request {
            v,
            id,
            method,
            params,
        } if v == PROTOCOL_VERSION && id % 2 == 1 => {
            let permit = match Arc::clone(&context.inbound_slots).try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    let response = LppMessage::failure(
                        id,
                        RpcError::new(
                            "HOST_BUSY",
                            format!(
                                "host allows at most {MAX_INBOUND_REQUESTS} concurrent plugin requests"
                            ),
                        ),
                    );
                    if context.outbound.send(response).await.is_err() {
                        tracing::warn!(request_id = id, "could not send host busy response");
                    }
                    return;
                }
            };
            let cancellation = CancellationToken::new();
            let duplicate = {
                let mut inbound = context.inbound.lock();
                match inbound.entry(id) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(cancellation.clone());
                        false
                    }
                    std::collections::hash_map::Entry::Occupied(_) => true,
                }
            };
            if duplicate {
                let response = LppMessage::failure(
                    id,
                    RpcError::new("DUPLICATE_REQUEST", "plugin reused an active request id"),
                );
                if context.outbound.send(response).await.is_err() {
                    tracing::warn!(request_id = id, "could not send duplicate request response");
                }
                return;
            }
            let outbound = context.outbound.clone();
            let services = context.services.clone();
            let inbound = Arc::clone(&context.inbound);
            tokio::spawn(async move {
                let work = tokio::task::spawn_blocking(move || {
                    dispatch_host_request(&method, params, &services)
                });
                let result = work.await.unwrap_or_else(|error| {
                    Err(PluginError::Rpc {
                        code: "HOST_TASK_FAILED".to_owned(),
                        message: error.to_string(),
                    })
                });
                let result = if cancellation.is_cancelled() {
                    Err(PluginError::Rpc {
                        code: "CANCELLED".to_owned(),
                        message: "plugin cancelled the host request".to_owned(),
                    })
                } else {
                    result
                };
                inbound.lock().remove(&id);
                drop(permit);
                let response = result
                    .map(|result| LppMessage::success(id, result))
                    .unwrap_or_else(|error| LppMessage::failure(id, plugin_error_to_rpc(error)));
                if outbound.send(response).await.is_err() {
                    tracing::debug!(request_id = id, "could not send host RPC response");
                }
            });
        }
        LppMessage::Event { v, method, .. } if v == PROTOCOL_VERSION => {
            tracing::debug!(%method, "received plugin event");
        }
        LppMessage::Cancel { v, request_id } if v == PROTOCOL_VERSION && request_id % 2 == 1 => {
            if let Some(cancellation) = context.inbound.lock().get(&request_id).cloned() {
                cancellation.cancel();
            } else {
                tracing::debug!(request_id, "plugin cancelled an inactive host request");
            }
        }
        _ => tracing::warn!("ignored invalid or wrong-direction LPP message"),
    }
}

pub(super) fn cancel_inbound(inbound: &InboundRequests) {
    for cancellation in std::mem::take(&mut *inbound.lock()).into_values() {
        cancellation.cancel();
    }
}

pub(super) fn fail_pending(pending: &PendingRequests) {
    let requests = std::mem::take(&mut *pending.lock());
    for (request_id, response_tx) in requests {
        if response_tx
            .send(Err(PluginError::ConnectionClosed))
            .is_err()
        {
            tracing::debug!(request_id, "closed plugin response receiver was dropped");
        }
    }
}
