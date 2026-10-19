use std::sync::Arc;

use lovelycarve_plugin::protocol::{self, Hello};
use tokio::io::{duplex, split};

use crate::core::evidence::partitions::SectorSource;

use super::*;

struct MemorySource(Vec<u8>);

impl SectorSource for MemorySource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.0.len());
        let end = start.saturating_add(len).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }

    fn size(&self) -> u64 {
        self.0.len() as u64
    }
}

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(key, value)| (Value::from(*key), value.clone()))
            .collect(),
    )
}

pub(in crate::core::extensions::plugin) fn hello(plugin: &str) -> Value {
    protocol::to_value(&Hello {
        protocol: PROTOCOL_VERSION,
        plugin: plugin.to_owned(),
        sdk: "0.2.0".to_owned(),
        analyzers: Vec::new(),
        filesystems: Vec::new(),
        records: Vec::new(),
        schema_hash: lovelycarve_plugin::schema::schema_hash(&[]),
    })
    .expect("hello")
}

#[tokio::test]
async fn nested_source_read_does_not_deadlock_plugin_call() {
    let sources = SourceRegistry::default();
    let memory_source: Arc<dyn SectorSource + Send + Sync> =
        Arc::new(MemorySource(b"abcdef".to_vec()));
    let source = sources.register(Arc::clone(&memory_source), 512);
    let (host_stream, plugin_stream) = duplex(16 * 1024);
    let (host_reader, host_writer) = split(host_stream);
    let (mut plugin_reader, mut plugin_writer) = split(plugin_stream);

    let plugin = tokio::spawn(async move {
        write_frame(
            &mut plugin_writer,
            &LppMessage::request(1, "plugin.hello", hello("org.lovelycarve.echo")),
        )
        .await
        .expect("send hello");
        let LppMessage::Response {
            id: 1,
            result: Some(answer),
            ..
        } = read_frame(&mut plugin_reader)
            .await
            .expect("hello response")
        else {
            panic!("expected the host hello");
        };
        let answer: protocol::HostHello = protocol::from_value(answer).expect("host hello");
        assert_eq!(answer.protocol, PROTOCOL_VERSION);

        let LppMessage::Request { id, method, .. } =
            read_frame(&mut plugin_reader).await.expect("host ping")
        else {
            panic!("expected ping request");
        };
        assert_eq!(method, "ping");
        write_frame(
            &mut plugin_writer,
            &LppMessage::request(
                3,
                "source.read",
                map(&[
                    ("source_id", Value::from(source.source_id)),
                    ("offset", Value::from(1)),
                    ("length", Value::from(3)),
                ]),
            ),
        )
        .await
        .expect("request source bytes");
        let LppMessage::Response {
            id: 3,
            result: Some(bytes),
            error: None,
            ..
        } = read_frame(&mut plugin_reader)
            .await
            .expect("source.read response")
        else {
            panic!("expected successful source response");
        };
        write_frame(&mut plugin_writer, &LppMessage::success(id, bytes))
            .await
            .expect("send ping response");
    });

    let client = PluginClient::connect(
        Box::new(host_reader),
        Box::new(host_writer),
        HostRpcServices::new(sources),
        None,
        None,
    )
    .await
    .expect("connect plugin");
    assert_eq!(client.hello().plugin, "org.lovelycarve.echo");
    let result = timeout(Duration::from_secs(1), client.call("ping", Value::Nil))
        .await
        .expect("nested RPC must not deadlock")
        .expect("ping succeeds");
    assert_eq!(result, Value::Binary(b"bcd".to_vec()));
    plugin.await.expect("plugin task");
}

#[tokio::test]
async fn timed_out_and_dropped_calls_send_cancel_and_clear_pending() {
    let (host_stream, plugin_stream) = duplex(16 * 1024);
    let (host_reader, host_writer) = split(host_stream);
    let (mut plugin_reader, mut plugin_writer) = split(plugin_stream);
    let plugin = tokio::spawn(async move {
        write_frame(
            &mut plugin_writer,
            &LppMessage::request(1, "plugin.hello", hello("org.test.timeout")),
        )
        .await
        .expect("hello");
        read_frame(&mut plugin_reader).await.expect("host hello");
        for _ in 0..2 {
            let LppMessage::Request { id, .. } =
                read_frame(&mut plugin_reader).await.expect("slow request")
            else {
                panic!("expected request");
            };
            assert!(matches!(
                read_frame(&mut plugin_reader).await.expect("cancel"),
                LppMessage::Cancel { request_id, .. } if request_id == id
            ));
        }
    });
    let client = PluginClient::connect(
        Box::new(host_reader),
        Box::new(host_writer),
        HostRpcServices::default(),
        None,
        None,
    )
    .await
    .expect("connect");
    assert!(matches!(
        client
            .call_with_timeout("slow", Value::Nil, Duration::from_millis(10))
            .await,
        Err(PluginError::Timeout)
    ));
    assert!(client.pending.lock().is_empty());
    // A caller that stops waiting (a cancelled task) cancels too.
    tokio::select! {
        _ = client.call("slow", Value::Nil) => panic!("the call must not finish"),
        () = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    assert!(client.pending.lock().is_empty());
    plugin.await.expect("plugin task");
}

#[tokio::test]
async fn lpp_v1_plugins_are_refused_at_the_handshake() {
    let (host_stream, plugin_stream) = duplex(16 * 1024);
    let (host_reader, host_writer) = split(host_stream);
    let (_plugin_reader, mut plugin_writer) = split(plugin_stream);
    let legacy = LppMessage::Request {
        v: 1,
        id: 1,
        method: "plugin.hello".to_owned(),
        params: map(&[
            ("protocol_min", Value::from(1)),
            ("protocol_max", Value::from(1)),
        ]),
    };
    write_frame(&mut plugin_writer, &legacy)
        .await
        .expect("hello");
    let Err(PluginError::Handshake(message)) = PluginClient::connect(
        Box::new(host_reader),
        Box::new(host_writer),
        HostRpcServices::default(),
        None,
        None,
    )
    .await
    else {
        panic!("an LPP v1 plugin must be refused");
    };
    assert!(message.contains("LPP 1"), "{message}");
}

#[test]
fn host_requests_without_a_run_are_refused() {
    let services = HostRpcServices::default();
    let read = protocol::to_value(&protocol::FileRead {
        file_id: "run_unknown.f1".to_owned(),
        offset: 0,
        length: 4,
    })
    .expect("params");
    assert!(matches!(
        super::host_requests::dispatch_host_request(protocol::method::FILE_READ, read, &services),
        Err(PluginError::Rpc { code, .. }) if code == "INVALID_HANDLE"
    ));
    assert!(matches!(
        super::host_requests::dispatch_host_request("evidence.read", Value::Nil, &services),
        Err(PluginError::Rpc { code, .. }) if code == "METHOD_NOT_FOUND"
    ));
}
