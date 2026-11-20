use super::inference::{Capabilities, OcrExecution};
use super::*;

unsafe extern "C" fn unused_call(_: Bytes, _: Bytes, _: Bytes, _: *mut Buffer) -> i32 {
    STATUS_OK
}

unsafe extern "C" fn unused_free(_: *mut Buffer) {}

/// Calls `serve_call` like a host would and returns the status and the
/// reply text, releasing the reply through `serve_free_buffer`.
fn call(
    method: &[u8],
    dispatch: impl FnOnce(&str, &[u8], &[u8]) -> Result<Vec<u8>, CallError>,
) -> (i32, String) {
    let mut out = Buffer::empty();
    let status = unsafe {
        serve_call(
            Bytes::from_slice(method),
            Bytes::from_slice(b"{}"),
            Bytes::from_slice(&[1, 2, 3]),
            &mut out,
            dispatch,
        )
    };
    let reply = String::from_utf8_lossy(unsafe { out.as_slice() }).into_owned();
    unsafe { serve_free_buffer(&mut out) };
    assert!(out.data.is_null());
    (status, reply)
}

#[test]
fn serve_call_passes_arguments_and_returns_the_reply() {
    let (status, reply) = call(b"demo.echo", |method, params, payload| {
        assert_eq!(
            (method, params, payload),
            ("demo.echo", &b"{}"[..], &[1, 2, 3][..])
        );
        Ok(b"\"done\"".to_vec())
    });
    assert_eq!((status, reply.as_str()), (STATUS_OK, "\"done\""));
}

#[test]
fn serve_call_maps_errors_to_statuses() {
    assert_eq!(
        call(b"demo.fail", |_, _, _| Err(CallError::Failed(
            "推理失败".to_string()
        ))),
        (STATUS_ERROR, "推理失败".to_string())
    );
    assert_eq!(
        call(b"demo.bad", |_, _, _| Err(CallError::InvalidArgument(
            "bad params".to_string()
        ))),
        (STATUS_INVALID_ARGUMENT, "bad params".to_string())
    );
    assert_eq!(
        call(&[0xff, 0xfe], |_, _, _| Ok(Vec::new())),
        (
            STATUS_INVALID_ARGUMENT,
            "component method name is not UTF-8".to_string()
        )
    );
}

#[test]
fn serve_call_turns_panics_into_error_replies() {
    let (status, reply) = call(b"demo.panic", |_, _, _| panic!("model exploded"));
    assert_eq!(
        (status, reply.as_str()),
        (STATUS_PANIC, "component panicked: model exploded")
    );
}

#[test]
fn serve_call_rejects_a_null_reply_buffer() {
    let status = unsafe {
        serve_call(
            Bytes::from_slice(b"demo.echo"),
            Bytes::from_slice(&[]),
            Bytes::from_slice(&[]),
            std::ptr::null_mut(),
            |_, _, _| Ok(Vec::new()),
        )
    };
    assert_eq!(status, STATUS_INVALID_ARGUMENT);
}

#[test]
fn serve_entry_writes_a_v1_table() {
    let mut api = std::mem::MaybeUninit::<ComponentApiV1>::uninit();
    let status = unsafe { serve_entry(api.as_mut_ptr(), unused_call, unused_free) };
    assert_eq!(status, STATUS_OK);
    let api = unsafe { api.assume_init() };
    assert_eq!(
        (api.abi_version, api.struct_size),
        (ABI_VERSION, COMPONENT_API_SIZE)
    );
    assert_eq!(
        unsafe { serve_entry(std::ptr::null_mut(), unused_call, unused_free) },
        STATUS_INVALID_ARGUMENT
    );
}

#[test]
fn description_json_shape_is_stable() {
    let description = Description {
        name: inference::LIBRARY.to_string(),
        version: "1.3.0".to_string(),
        capabilities: Capabilities {
            file_types: true,
            ocr: Some(OcrExecution::Cuda),
        },
    };
    let json = serde_json::to_string(&description).expect("serialize description");
    assert_eq!(
        json,
        r#"{"name":"lovelycarve_inference","version":"1.3.0","capabilities":{"fileTypes":true,"ocr":"cuda"}}"#
    );
    let parsed: Description<Capabilities> = serde_json::from_str(&json).expect("parse description");
    assert_eq!(parsed, description);
}
