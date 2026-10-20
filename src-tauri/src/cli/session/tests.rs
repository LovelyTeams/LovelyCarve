use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use super::*;

#[test]
fn references_are_structural_and_wait_only_for_pending_dependencies() {
    let results = HashMap::from([("open".to_owned(), Json::from(json!({"id":"image-1"})))]);
    let pending = HashSet::from(["open".to_owned()]);
    let mut args = json!({"imageId":{"$ref":"open#/id"},"text":"$open/id"});
    assert!(pending_ref(&args, &pending));
    resolve_refs(&mut args, &results).unwrap();
    assert_eq!(args, json!({"imageId":"image-1","text":"$open/id"}));
    assert!(!pending_ref(&args, &pending));
    assert!(resolve_refs(&mut json!({"$ref":"missing#/id"}), &results).is_err());
    assert!(resolve_refs(&mut json!({"$ref":true}), &results).is_err());
    assert!(resolve_refs(&mut json!({"$ref":"open#/missing"}), &results).is_err());
}

#[test]
fn responses_carry_results_in_the_order_the_command_wrote_them() {
    #[derive(serde::Serialize)]
    struct Image {
        id: &'static str,
        bytes: u64,
    }
    let result = Json::serialize(&Image { id: "x", bytes: 3 });
    assert_eq!(
        response(Some("open"), &result).unwrap(),
        r#"{"id":"open","result":{"id":"x","bytes":3}}"#
    );
    assert_eq!(
        response(None, &Err(CliError::invalid("bad"))).unwrap(),
        r#"{"id":null,"error":{"code":"invalidInput","message":"bad"}}"#
    );
}

#[test]
fn mutations_and_unknown_commands_are_barriers() {
    for command in [
        "images open",
        "images close",
        "cases open",
        "cases close",
        "context set",
        "context use",
        "tasks wait",
        "indexes delete",
        "searches delete",
        "future command",
    ] {
        assert!(!concurrent(command), "{command}");
    }
    for command in [
        "files list",
        "files read",
        "files hash",
        "files search",
        "tasks cancel",
        "images get",
    ] {
        assert!(concurrent(command), "{command}");
    }
}

#[test]
fn arguments_use_the_same_api_names_and_validation_as_body() {
    let catalog = catalog::catalog().unwrap();
    let read = catalog.find_path("files read").unwrap();
    let args = json!({"nodeId": "n", "imageId": "i", "offset": 1, "length": 2});
    let Value::Object(args) = args else {
        unreachable!()
    };
    read.validate(&args).unwrap();
    let Value::Object(typo) = json!({"typo": true}) else {
        unreachable!()
    };
    assert!(read.validate(&typo).is_err());
    for invalid in [
        json!({"node_id": "n"}),
        json!({"offset": "1"}),
        json!({"offset": -1}),
        json!({"path": "a", "nodeId": "n"}),
    ] {
        assert!(read.validate(invalid.as_object().unwrap()).is_err());
    }
}

#[tokio::test]
async fn queries_overlap_flush_with_open_input_and_barriers_preserve_context() {
    let (sender, receiver) = mpsc::channel(8);
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let release_receiver = Arc::new(Mutex::new(release_receiver));
    let (finished_sender, mut finished_receiver) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicUsize::new(0));
    let executor = {
        let active = active.clone();
        move |context: &mut Context, request: Request| match request.id.as_str() {
            "slow" => {
                active.fetch_add(1, Ordering::SeqCst);
                finished_sender.send("started").unwrap();
                release_receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(json!({"id":"source"}).into())
            }
            "fast" => {
                assert_eq!(context.image_id.as_deref(), Some("initial"));
                finished_sender.send("fast").unwrap();
                Ok(Value::Null.into())
            }
            "dependent" => {
                assert_eq!(request.args["imageId"], "source");
                Ok(Value::Null.into())
            }
            "switch" => {
                assert_eq!(active.load(Ordering::SeqCst), 0);
                context.image_id = Some("next".into());
                Ok(Value::Null.into())
            }
            "after" => {
                assert_eq!(context.image_id.as_deref(), Some("next"));
                finished_sender.send("after").unwrap();
                Ok(Value::Null.into())
            }
            _ => panic!("unexpected request"),
        }
    };
    let worker = tokio::spawn(async move {
        let mut context = Context {
            image_id: Some("initial".into()),
            ..Context::default()
        };
        let failed = run_lines(&mut context, receiver, 2, executor)
            .await
            .unwrap();
        assert!(!failed);
        assert_eq!(context.image_id.as_deref(), Some("next"));
    });
    sender
        .send(Ok(json!({"id":"slow","command":"images get"}).to_string()))
        .await
        .unwrap();
    assert_eq!(finished_receiver.recv().await.unwrap(), "started");
    sender
        .send(Ok(json!({"id":"fast","command":"tasks list"}).to_string()))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), finished_receiver.recv())
            .await
            .unwrap()
            .unwrap(),
        "fast"
    );
    assert!(!worker.is_finished()); // stdin is still open; slow query is still running.
    for request in [
        json!({"id":"dependent","command":"images get","args":{"imageId":{"$ref":"slow#/id"}}}),
        json!({"id":"switch","command":"context set"}),
        json!({"id":"after","command":"context get"}),
    ] {
        sender.send(Ok(request.to_string())).await.unwrap();
    }
    release_sender.send(()).unwrap();
    assert_eq!(finished_receiver.recv().await.unwrap(), "after");
    drop(sender);
    worker.await.unwrap();
}

#[tokio::test]
async fn failed_dependencies_duplicates_and_panics_do_not_stop_other_queries() {
    let (sender, receiver) = mpsc::channel(12);
    for line in [
        "invalid JSON".to_owned(),
        json!({"id":"bad","command":"images get"}).to_string(),
        json!({"id":"dependent","command":"images get","args":{"imageId":{"$ref":"bad#/id"}}})
            .to_string(),
        json!({"id":"bad","command":"images get"}).to_string(),
        json!({"id":"panic","command":"tasks list"}).to_string(),
        json!({"id":"good","command":"tasks list"}).to_string(),
    ] {
        sender.send(Ok(line)).await.unwrap();
    }
    drop(sender);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let observed = executed.clone();
    assert!(
        run_lines(&mut Context::default(), receiver, 2, move |_, request| {
            observed.lock().unwrap().push(request.id.clone());
            match request.id.as_str() {
                "bad" => Err(CliError::invalid("failed")),
                "panic" => panic!("test handler panic"),
                "good" => Ok(Value::Null.into()),
                _ => panic!("invalid request reached executor"),
            }
        })
        .await
        .unwrap()
    );
    let mut executed = executed.lock().unwrap().clone();
    executed.sort();
    assert_eq!(executed, ["bad", "good", "panic"]);
}

#[tokio::test]
async fn sequential_default_and_parallel_limit_bound_active_queries() {
    for jobs in [1, 2, 4] {
        let (sender, receiver) = mpsc::channel(12);
        for index in 0..10 {
            sender
                .send(Ok(
                    json!({"id":index.to_string(),"command":"images get"}).to_string()
                ))
                .await
                .unwrap();
        }
        drop(sender);
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let observed = maximum.clone();
        assert!(
            !run_lines(&mut Context::default(), receiver, jobs, move |_, _| {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                observed.fetch_max(count, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(Value::Null.into())
            })
            .await
            .unwrap()
        );
        assert!(maximum.load(Ordering::SeqCst) <= jobs);
    }
}
