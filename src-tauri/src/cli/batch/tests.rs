use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use super::*;

#[test]
fn references_are_structural_and_wait_only_for_pending_dependencies() {
    let results = HashMap::from([(
        "raid".to_owned(),
        Json::from(json!({"assembled":[{"path":"/md0"}]})),
    )]);
    let pending = HashSet::from(["raid".to_owned()]);
    let mut argv = json!(["ls", {"$ref":"raid#/assembled/0/path"}, "$raid/x"]);
    assert!(pending_ref(&argv, &pending));
    resolve_refs(&mut argv, &results).unwrap();
    assert_eq!(argv, json!(["ls", "/md0", "$raid/x"]));
    assert!(!pending_ref(&argv, &pending));
    assert!(resolve_refs(&mut json!({"$ref":"missing#/id"}), &results).is_err());
    assert!(resolve_refs(&mut json!({"$ref":true}), &results).is_err());
    assert!(resolve_refs(&mut json!({"$ref":"raid#/missing"}), &results).is_err());
    assert_eq!(
        words(vec![json!("ls"), json!(5), json!(true), json!(["a"])]).unwrap(),
        ["ls", "5", "true", "[\"a\"]"]
    );
    assert!(words(vec![Value::Null]).is_err());
}

#[test]
fn responses_carry_results_in_the_order_the_command_wrote_them() {
    #[derive(serde::Serialize)]
    struct Image {
        id: &'static str,
        bytes: u64,
    }
    let result = Json::serialize(&Image { id: "x", bytes: 3 }).map(Answer::from);
    assert_eq!(
        response(Some("open"), &result).unwrap(),
        r#"{"id":"open","result":{"id":"x","bytes":3}}"#
    );
    assert_eq!(
        response(None, &Err(CliError::invalid("bad"))).unwrap(),
        r#"{"id":null,"error":{"code":"invalidInput","message":"bad"}}"#
    );
    // An incomplete result keeps its data and says why it is incomplete.
    let hash = prepare_command(&["hash", "/x/a"]);
    let output = Output::json(json!({"items": [{"sha256": "x", "readFaultsDetected": true}]}));
    let answer = answer(&hash, output, None, &[]).unwrap();
    assert_eq!(
        response(Some("digest"), &Ok(answer)).unwrap(),
        r#"{"id":"digest","result":{"items":[{"readFaultsDetected":true,"sha256":"x"}]},"warning":{"code":"incompleteResult","message":"结果不完整：items.readFaultsDetected = true。结果仍会返回，但不能当作完整结果使用。","details":{"fields":{"items.readFaultsDetected":true}}}}"#
    );
}

fn prepare_command(argv: &[&str]) -> Command {
    let words: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
    match prepare("x".into(), &words, None).unwrap() {
        Ok(prepared) => prepared.command,
        Err(_) => panic!("answered without running"),
    }
}

#[test]
fn requests_are_command_lines_without_process_options() {
    assert_eq!(
        prepare_command(&["ls", "/nas/part:0", "--limit", "5"]).path(),
        "ls"
    );
    let words = |argv: &[&str]| {
        argv.iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    for refused in [
        &["-C", "D:/case", "ls", "/"][..],
        &["--image", "D:/x.E01", "ls", "/"],
        &["shell"],
        &["batch"],
        &["case", "open", "D:/case"],
        &["unlock", "/nas/part:2", "--password", "-"],
        &["unlock", "/nas/part:2", "--password"],
    ] {
        assert!(
            prepare("x".into(), &words(refused), None).is_err(),
            "{refused:?}"
        );
    }
    // Commands that need no evidence answer at once.
    assert!(matches!(
        prepare("x".into(), &words(&["schema", "ls"]), None),
        Ok(Err(_))
    ));
    assert!(matches!(
        prepare("x".into(), &words(&["--dry-run", "ls", "/"]), None),
        Ok(Err(_))
    ));
    // --json=fields projects a list result, and is refused for raw content.
    let Ok(Ok(prepared)) = prepare("x".into(), &words(&["--json=name", "ls", "/x"]), None) else {
        panic!("ls with fields")
    };
    let page = Output::json(json!({"items": [{"name": "a", "size": 1}], "nextCursor": null}));
    let projected = answer(&prepared.command, page, None, &prepared.fields).unwrap();
    assert_eq!(
        projected.json.value(),
        &json!({"items": [{"name": "a"}], "nextCursor": null})
    );
    assert!(prepare("x".into(), &words(&["--json=name", "cat", "/x/a"]), None).is_err());
    assert!(prepare("x".into(), &words(&["--json=a,,b", "ls", "/x"]), None).is_err());
}

#[test]
fn mutations_are_barriers_and_queries_may_overlap() {
    for argv in [
        &["evidence", "add", "D:/x.E01"][..],
        &["evidence", "close", "x"],
        &["case", "lock"],
        &["tasks", "wait"],
        &["index", "rm", "/x/part:0"],
        &["raid", "detect"],
        &["export", "/x/a", "-o", "D:/out"],
    ] {
        assert!(!prepare_command(argv).meta().concurrent, "{argv:?}");
    }
    for argv in [
        &["ls", "/"][..],
        &["cat", "/x/a"],
        &["hash", "/x/a"],
        &["grep", "secret", "/x"],
        &["evidence", "show"],
        &["tasks"],
    ] {
        assert!(prepare_command(argv).meta().concurrent, "{argv:?}");
    }
}

fn request(id: &str, argv: &[&str]) -> String {
    json!({"id": id, "argv": argv}).to_string()
}

#[tokio::test]
async fn the_status_reports_failures_before_incomplete_results() {
    for (requests, expected) in [
        (vec!["clean"], 0),
        (vec!["clean", "partial"], EXIT_INCOMPLETE),
        (vec!["partial", "bad"], 1),
    ] {
        let (sender, receiver) = mpsc::channel(4);
        for id in &requests {
            sender
                .send(Ok(request(id, &["hash", "/x/a"])))
                .await
                .unwrap();
        }
        drop(sender);
        let status = run_lines(receiver, 1, std::io::sink(), move |prepared| {
            let faults = match prepared.id.as_str() {
                "clean" => false,
                "partial" => true,
                _ => return Err(CliError::invalid("failed")),
            };
            let output = Output::json(json!({"items": [{"readFaultsDetected": faults}]}));
            answer(&prepared.command, output, None, &prepared.fields)
        })
        .await
        .unwrap();
        assert_eq!(status, expected, "{requests:?}");
    }
}

#[tokio::test]
async fn queries_overlap_flush_with_open_input_and_barriers_wait() {
    let (sender, receiver) = mpsc::channel(8);
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let release_receiver = Arc::new(Mutex::new(release_receiver));
    let (finished_sender, mut finished_receiver) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicUsize::new(0));
    let executor = {
        let active = active.clone();
        move |prepared: Prepared| match prepared.id.as_str() {
            "slow" => {
                active.fetch_add(1, Ordering::SeqCst);
                finished_sender.send("started").unwrap();
                release_receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(Json::from(json!({"path":"/source"})).into())
            }
            "fast" => {
                finished_sender.send("fast").unwrap();
                Ok(Json::from(Value::Null).into())
            }
            "dependent" => {
                let Command::Ls(args) = &prepared.command else {
                    panic!("ls expected")
                };
                assert_eq!(args.path.as_ref().unwrap().to_string(), "/source");
                Ok(Json::from(Value::Null).into())
            }
            "barrier" => {
                assert_eq!(active.load(Ordering::SeqCst), 0);
                Ok(Json::from(Value::Null).into())
            }
            "after" => {
                finished_sender.send("after").unwrap();
                Ok(Json::from(Value::Null).into())
            }
            _ => panic!("unexpected request"),
        }
    };
    let worker = tokio::spawn(async move {
        let status = run_lines(receiver, 2, std::io::sink(), executor)
            .await
            .unwrap();
        assert_eq!(status, 0);
    });
    sender
        .send(Ok(request("slow", &["stat", "/x"])))
        .await
        .unwrap();
    assert_eq!(finished_receiver.recv().await.unwrap(), "started");
    sender.send(Ok(request("fast", &["tasks"]))).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), finished_receiver.recv())
            .await
            .unwrap()
            .unwrap(),
        "fast"
    );
    assert!(!worker.is_finished()); // stdin is still open; slow query is still running.
    sender
        .send(Ok(
            json!({"id":"dependent","argv":["ls", {"$ref":"slow#/path"}]}).to_string(),
        ))
        .await
        .unwrap();
    sender
        .send(Ok(request("barrier", &["tasks", "wait"])))
        .await
        .unwrap();
    sender.send(Ok(request("after", &["tasks"]))).await.unwrap();
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
        request("bad", &["stat", "/x"]),
        json!({"id":"dependent","argv":["ls", {"$ref":"bad#/path"}]}).to_string(),
        request("bad", &["stat", "/x"]),
        request("usage", &["ls", "--nope"]),
        request("panic", &["tasks"]),
        request("good", &["tasks"]),
    ] {
        sender.send(Ok(line)).await.unwrap();
    }
    drop(sender);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let observed = executed.clone();
    let mut out = Vec::new();
    assert_eq!(
        run_lines(receiver, 2, &mut out, move |prepared| {
            observed.lock().unwrap().push(prepared.id.clone());
            match prepared.id.as_str() {
                "bad" => Err(CliError::invalid("failed")),
                "panic" => panic!("test handler panic"),
                "good" => Ok(Json::from(Value::Null).into()),
                _ => panic!("invalid request reached executor"),
            }
        })
        .await
        .unwrap(),
        1
    );
    let mut executed = executed.lock().unwrap().clone();
    executed.sort();
    assert_eq!(executed, ["bad", "good", "panic"]);
    // Every line gets exactly one response, under its ID when it has one.
    let mut outcomes: Vec<(Value, String)> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|line| {
            let response: Value = serde_json::from_str(line).unwrap();
            let outcome = response["error"]["code"]
                .as_str()
                .unwrap_or("ok")
                .to_owned();
            (response["id"].clone(), outcome)
        })
        .collect();
    outcomes.sort_by_key(|(id, outcome)| (id.to_string(), outcome.clone()));
    assert_eq!(
        outcomes,
        [
            (json!("bad"), "invalidInput".to_owned()),
            (json!("bad"), "invalidInput".to_owned()),
            (json!("dependent"), "invalidInput".to_owned()),
            (json!("good"), "ok".to_owned()),
            (json!("panic"), "internal".to_owned()),
            (json!("usage"), "usage".to_owned()),
            (Value::Null, "invalidInput".to_owned()),
        ]
    );
}

#[tokio::test]
async fn sequential_default_and_parallel_limit_bound_active_queries() {
    for jobs in [1, 2, 4] {
        let (sender, receiver) = mpsc::channel(12);
        for index in 0..10 {
            sender
                .send(Ok(request(&index.to_string(), &["stat", "/x"])))
                .await
                .unwrap();
        }
        drop(sender);
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let observed = maximum.clone();
        assert_eq!(
            run_lines(receiver, jobs, std::io::sink(), move |_| {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                observed.fetch_max(count, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(Json::from(Value::Null).into())
            })
            .await
            .unwrap(),
            0
        );
        assert!(maximum.load(Ordering::SeqCst) <= jobs);
    }
}
