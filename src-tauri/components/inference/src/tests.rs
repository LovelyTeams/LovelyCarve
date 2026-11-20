use abi::inference::{
    Capabilities, ContentType, IdentifyParams, METHOD_IDENTIFY, METHOD_RECOGNIZE,
    SAMPLE_WINDOW as WINDOW,
};
use abi::{Buffer, Bytes, CallError, ComponentApiV1, Description};

use super::*;

/// Builds the identify params and payload the host sends for `content`.
fn sampled(content: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let head = &content[..content.len().min(WINDOW)];
    let tail = &content[content.len() - content.len().min(WINDOW)..];
    let params = serde_json::to_vec(&IdentifyParams {
        head_len: head.len() as u64,
        file_len: content.len() as u64,
    })
    .expect("encode params");
    (params, [head, tail].concat())
}

fn identify(content: &[u8]) -> ContentType {
    let (params, payload) = sampled(content);
    let reply = dispatch(METHOD_IDENTIFY, &params, &payload).expect("identify");
    serde_json::from_slice(&reply).expect("decode content type")
}

fn invalid_argument(result: Result<Vec<u8>, CallError>) -> String {
    match result {
        Err(CallError::InvalidArgument(message)) => message,
        other => panic!("expected an invalid argument, got {other:?}"),
    }
}

#[test]
fn describe_reports_this_build() {
    let reply = dispatch(abi::METHOD_DESCRIBE, b"", b"").expect("describe");
    let description: Description<Capabilities> =
        serde_json::from_slice(&reply).expect("decode description");
    assert_eq!(
        description,
        Description {
            name: "lovelycarve_inference".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: Capabilities {
                file_types: true,
                ocr: ocr_execution(),
            },
        }
    );
}

#[test]
fn identifies_content_regardless_of_its_name() {
    // The CLI smoke test stores this script as `signature.bin`.
    let script =
        b"#!/usr/bin/env python3\nimport sys\nprint('LovelyCarve forensic CLI')\n".repeat(20);
    let content_type = identify(&script);
    assert_eq!(
        (
            content_type.label.as_str(),
            content_type.is_text,
            content_type.inferred
        ),
        ("python", true, true)
    );
    assert!(content_type.score > 0.5, "{content_type:?}");
}

#[test]
fn larger_files_send_only_their_end_windows() {
    let mut script = b"#!/usr/bin/env python3\nimport sys\n".to_vec();
    script.extend(b"print('LovelyCarve forensic CLI')\n".repeat(600));
    assert!(script.len() > 2 * WINDOW);
    let (_, payload) = sampled(&script);
    assert_eq!(payload.len(), 2 * WINDOW);
    assert_eq!(identify(&script).label, "python");
}

#[test]
fn empty_content_is_a_rule_decision() {
    assert_eq!(
        identify(b""),
        ContentType {
            label: "empty".to_string(),
            mime_type: "inode/x-empty".to_string(),
            group: "inode".to_string(),
            description: "Empty file".to_string(),
            extensions: Vec::new(),
            is_text: false,
            score: 1.0,
            inferred: false,
        }
    );
}

#[test]
fn identify_rejects_inconsistent_windows() {
    let params = |head_len: u64, file_len: u64| {
        serde_json::to_vec(&IdentifyParams { head_len, file_len }).expect("encode params")
    };
    let message = invalid_argument(dispatch(METHOD_IDENTIFY, &params(9, 9), b"short"));
    assert!(message.contains("exceeds the 5-byte payload"), "{message}");
    let message = invalid_argument(dispatch(METHOD_IDENTIFY, &params(4, 4), b"headtail+"));
    assert!(message.contains("exceed the 4-byte file"), "{message}");
    let message = invalid_argument(dispatch(METHOD_IDENTIFY, b"[]", b""));
    assert!(
        message.starts_with("magika.identify params are invalid"),
        "{message}"
    );
}

#[test]
fn unknown_methods_are_invalid_arguments() {
    let message = invalid_argument(dispatch("magika.train", b"{}", b""));
    assert_eq!(
        message,
        "lovelycarve_inference does not implement method `magika.train`"
    );
}

#[cfg(not(feature = "ocr"))]
#[test]
fn recognize_fails_clearly_without_ocr() {
    match dispatch(METHOD_RECOGNIZE, b"{}", b"") {
        Err(CallError::Failed(message)) => assert_eq!(
            message,
            "this build of lovelycarve_inference has no OCR (build it with the `ocr` feature)"
        ),
        other => panic!("expected an OCR failure, got {other:?}"),
    }
}

#[cfg(feature = "ocr")]
#[test]
fn recognize_rejects_missing_model_paths() {
    let message = invalid_argument(dispatch(METHOD_RECOGNIZE, b"{}", b""));
    assert!(
        message.starts_with("ocr.recognize params are invalid"),
        "{message}"
    );
}

#[test]
fn entry_point_serves_calls_through_the_c_abi() {
    let mut api = std::mem::MaybeUninit::<ComponentApiV1>::uninit();
    let status = unsafe { lovelycarve_component_v1(api.as_mut_ptr()) };
    assert_eq!(status, abi::STATUS_OK);
    let api = unsafe { api.assume_init() };

    let (params, payload) = sampled(b"");
    let mut out = Buffer::empty();
    let status = unsafe {
        (api.call)(
            Bytes::from_slice(METHOD_IDENTIFY.as_bytes()),
            Bytes::from_slice(&params),
            Bytes::from_slice(&payload),
            &mut out,
        )
    };
    let reply: ContentType =
        serde_json::from_slice(unsafe { out.as_slice() }).expect("decode reply");
    unsafe { (api.free_buffer)(&mut out) };
    assert_eq!((status, reply.label.as_str()), (abi::STATUS_OK, "empty"));
}
