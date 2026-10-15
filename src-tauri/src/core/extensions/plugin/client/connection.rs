//! LPP connection initialization and hello negotiation.
use super::*;

impl PluginClient {
    pub(in crate::core::extensions::plugin) async fn connect(
        mut reader: BoxReader,
        mut writer: BoxWriter,
        services: HostRpcServices,
        child: Option<Child>,
        progress_sink: Option<PluginProgressSink>,
        process_guard: Option<ProcessGuard>,
    ) -> Result<Arc<Self>> {
        let first_message = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader))
            .await
            .map_err(|_| PluginError::Timeout)??;
        let (request_id, hello) = parse_hello(first_message)?;
        let response = HostHello {
            protocol: PROTOCOL_VERSION,
            host: HostIdentity {
                name: "LovelyCarve",
                version: env!("CARGO_PKG_VERSION"),
            },
            limits: HostLimits {
                max_frame: MAX_FRAME_SIZE,
                max_read: crate::core::extensions::plugin::source_registry::MAX_SOURCE_READ,
                max_readv_ranges:
                    crate::core::extensions::plugin::source_registry::MAX_SOURCE_READV_RANGES,
            },
        };
        let response = rmpv::ext::to_value(response)
            .map_err(|error| PluginError::Handshake(error.to_string()))?;
        write_frame(&mut writer, &LppMessage::success(request_id, response)).await?;

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
            services,
            plugin_id: hello.plugin.id.clone(),
            progress_sink,
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
                        tracing::warn!("plugin reader stopped: {error}");
                        fail_pending(&router.pending);
                        cancel_inbound(&router.inbound);
                        break;
                    }
                }
            }
            reader_alive.store(false, Ordering::Release);
        });

        Ok(Arc::new(Self {
            identity: hello.plugin,
            capabilities: hello.capabilities,
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

pub(super) fn parse_hello(message: LppMessage) -> Result<(u64, PluginHello)> {
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
    if v != PROTOCOL_VERSION || id % 2 == 0 || method != "plugin.hello" {
        return Err(PluginError::Handshake(
            "first message must be an odd-id LPP v1 plugin.hello request".to_owned(),
        ));
    }
    Ok((id, PluginHello::from_params(params)?))
}
