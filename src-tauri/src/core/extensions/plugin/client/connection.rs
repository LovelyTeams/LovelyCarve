//! LPP connection setup: the `plugin.hello` handshake, then the reader and
//! writer tasks.
use lovelycarve_plugin::protocol::{self, HostHello};

use super::*;

impl PluginClient {
    /// Completes the handshake on `reader` / `writer` and starts serving.
    /// The plugin speaks first: an odd-id `plugin.hello` request whose
    /// payload is its [`Hello`]; only protocol 2 is accepted.
    pub(in crate::core::extensions::plugin) async fn connect(
        mut reader: BoxReader,
        mut writer: BoxWriter,
        services: HostRpcServices,
        child: Option<Child>,
        process_guard: Option<ProcessGuard>,
    ) -> Result<Arc<Self>> {
        let first_message = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader))
            .await
            .map_err(|_| PluginError::Timeout)??;
        let (request_id, hello) = parse_hello(first_message)?;
        let answer = protocol::to_value(&HostHello {
            protocol: PROTOCOL_VERSION,
            host: "LovelyCarve".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
        .map_err(|error| PluginError::Handshake(error.to_string()))?;
        write_frame(&mut writer, &LppMessage::success(request_id, answer)).await?;

        let alive = Arc::new(AtomicBool::new(true));
        let writer_alive = Arc::clone(&alive);
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<LppMessage>(64);
        tokio::spawn(async move {
            while let Some(message) = outbound_rx.recv().await {
                if let Err(error) = write_frame(&mut writer, &message).await {
                    tracing::warn!("plugin writer stopped: {error}");
                    break;
                }
            }
            writer_alive.store(false, Ordering::Release);
        });

        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let inbound: InboundRequests = Arc::new(Mutex::new(HashMap::new()));
        let inbound_slots = Arc::new(Semaphore::new(MAX_INBOUND_REQUESTS));
        let reader_alive = Arc::clone(&alive);
        let router = RouterContext {
            pending: Arc::clone(&pending),
            outbound: outbound_tx.clone(),
            services: services.clone(),
            inbound,
            inbound_slots,
        };
        tokio::spawn(async move {
            loop {
                match read_frame(&mut reader).await {
                    Ok(message) => {
                        route_message(message, &router).await;
                    }
                    Err(error) => {
                        tracing::debug!("plugin reader stopped: {error}");
                        fail_pending(&router.pending);
                        cancel_inbound(&router.inbound);
                        router.services.cancel_runs();
                        break;
                    }
                }
            }
            reader_alive.store(false, Ordering::Release);
        });

        Ok(Arc::new(Self {
            hello,
            services,
            outbound: outbound_tx,
            pending,
            next_request_id: AtomicU64::new(2),
            alive,
            runtime: tokio::runtime::Handle::current(),
            child: child.map(tokio::sync::Mutex::new),
            outbound_slots: Arc::new(Semaphore::new(MAX_OUTBOUND_REQUESTS)),
            _process_guard: process_guard,
            stderr_task: tokio::sync::Mutex::new(None),
        }))
    }
}

pub(super) fn parse_hello(message: LppMessage) -> Result<(u64, Hello)> {
    let LppMessage::Request {
        v,
        id,
        method,
        params,
    } = message
    else {
        return Err(PluginError::Handshake(
            "first message must be a plugin.hello request".to_owned(),
        ));
    };
    if id % 2 == 0 || method != protocol::method::HELLO {
        return Err(PluginError::Handshake(
            "first message must be an odd-id plugin.hello request".to_owned(),
        ));
    }
    if v != PROTOCOL_VERSION {
        return Err(PluginError::Handshake(format!(
            "the plugin speaks LPP {v}; LovelyCarve speaks LPP {PROTOCOL_VERSION} \
             (rebuild the plugin with SDK 0.2)"
        )));
    }
    let hello: Hello = protocol::from_value(params)
        .map_err(|error| PluginError::Handshake(format!("invalid plugin.hello: {error}")))?;
    if hello.protocol != PROTOCOL_VERSION {
        return Err(PluginError::Handshake(format!(
            "the plugin declares LPP {}; LovelyCarve speaks LPP {PROTOCOL_VERSION}",
            hello.protocol
        )));
    }
    Ok((id, hello))
}
