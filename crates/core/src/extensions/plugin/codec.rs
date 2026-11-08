use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub use lovelycarve_plugin::wire::MAX_FRAME_SIZE;

use super::{error::Result, LppMessage, PluginError};

pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<LppMessage> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await?;
    let payload_len = u32::from_be_bytes(header) as usize;
    if payload_len == 0 || payload_len > MAX_FRAME_SIZE {
        return Err(PluginError::InvalidFrame(format!(
            "payload length {payload_len} is outside 1..={MAX_FRAME_SIZE}"
        )));
    }

    let mut payload = vec![0_u8; payload_len];
    reader.read_exact(&mut payload).await?;
    Ok(rmp_serde::from_slice(&payload)?)
}

pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &LppMessage,
) -> Result<()> {
    let mut payload = rmp_serde::to_vec_named(message)?;
    // A host service can exceed the frame budget (e.g. legacy directory
    // enumeration). Fail this RPC without tearing down every other call.
    if payload.len() > MAX_FRAME_SIZE {
        if let LppMessage::Response { id, .. } = message {
            payload = rmp_serde::to_vec_named(&LppMessage::failure(
                *id,
                super::RpcError::new(
                    "RESOURCE_LIMIT",
                    "response exceeds the LPP frame limit; use smaller pages or reads",
                ),
            ))?;
        }
    }
    if payload.is_empty() || payload.len() > MAX_FRAME_SIZE {
        return Err(PluginError::InvalidFrame(format!(
            "encoded payload length {} is outside 1..={MAX_FRAME_SIZE}",
            payload.len()
        )));
    }
    let payload_len = u32::try_from(payload.len())
        .map_err(|_| PluginError::InvalidFrame("encoded payload is too large".to_owned()))?;
    writer.write_all(&payload_len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rmpv::Value;
    use tokio::io::duplex;

    use super::*;

    #[tokio::test]
    async fn oversized_response_fails_only_that_rpc() {
        let (mut writer, mut reader) = duplex(1024);
        let writing = tokio::spawn(async move {
            write_frame(
                &mut writer,
                &LppMessage::success(3, Value::Binary(vec![0; MAX_FRAME_SIZE])),
            )
            .await
            .expect("limit response");
            write_frame(&mut writer, &LppMessage::success(5, Value::from("next")))
                .await
                .expect("next response");
        });
        let response = read_frame(&mut reader).await.expect("failure frame");
        assert!(
            matches!(response, LppMessage::Response { id: 3, error: Some(error), .. } if error.code == "RESOURCE_LIMIT")
        );
        assert_eq!(
            read_frame(&mut reader).await.expect("next frame"),
            LppMessage::success(5, Value::from("next"))
        );
        writing.await.expect("writer");
    }

    #[tokio::test]
    async fn frame_round_trip_preserves_binary_values() {
        let (mut client, mut server) = duplex(1024);
        let expected = LppMessage::success(2, Value::Binary(vec![0, 1, 0xff]));
        let write = tokio::spawn({
            let expected = expected.clone();
            async move { write_frame(&mut client, &expected).await }
        });

        let actual = read_frame(&mut server).await.expect("read frame");
        write.await.expect("writer task").expect("write frame");
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_allocation() {
        let (mut client, mut server) = duplex(16);
        tokio::spawn(async move {
            client
                .write_all(&((MAX_FRAME_SIZE + 1) as u32).to_be_bytes())
                .await
                .expect("write header");
        });

        let error = read_frame(&mut server).await.expect_err("reject frame");
        assert!(matches!(error, PluginError::InvalidFrame(_)));
    }

    #[test]
    fn async_frames_match_the_loader_codec() {
        let message = LppMessage::request(3, "source.read", Value::Nil);
        let frame = lovelycarve_plugin::wire::encode_frame(&message).expect("encode");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let decoded = runtime
            .block_on(read_frame(&mut frame.as_slice()))
            .expect("decode");
        assert_eq!(decoded, message);
    }
}
