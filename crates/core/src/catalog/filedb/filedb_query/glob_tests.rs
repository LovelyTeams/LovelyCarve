use super::*;
use crate::catalog::filedb::filedb_store::test_support::*;
use crate::filesystem::tree::fs_tree::NodeKind;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("fixture directory");
    let entries = [
        ("1", "AUTH.LOG", "/var/log/AUTH.LOG"),
        ("2", "auth.log.1", "/var/log/auth.log.1"),
        ("3", "auth.log.2", "/var/log/auth.log.2"),
        ("4", "auth.log.12", "/var/log/auth.log.12"),
        ("5", "auth.log.3", "/tmp/auth.log.3"),
        ("6", "xauth.log.1", "/var/log/xauth.log.1"),
        ("7", "auth%.log", "/var/log/auth%.log"),
        ("8", "auth_.log", "/var/log/auth_.log"),
        ("9", "日志甲.LOG", "/var/log/日志甲.LOG"),
        ("10", "日志甲乙.log", "/var/log/日志甲乙.log"),
        ("11", "report[1].txt", "/reports/report[1].txt"),
        ("12", "auth' OR 1=1 --.log", "/var/log/auth' OR 1=1 --.log"),
    ]
    .map(|(id, name, path)| {
        entry(
            "part:0",
            &format!("ntfs:part:0:{id}"),
            name,
            NodeKind::File,
            path,
        )
    });
    let index = build_test_index(directory.path(), "part:0", &entries);
    (directory, index)
}

#[test]
fn glob_filters_before_pagination_and_combines_with_directory_scope() {
    let (_directory, index) = fixture();
    let page = |offset| {
        query_files(
            &index,
            &FileFilter {
                name_glob: Some("AUTH*.LOG.[12]".into()),
                path_prefix: Some("/VAR/LOG/".into()),
                kinds: vec!["file".into()],
                limit: 1,
                offset,
                ..FileFilter::default()
            }
            .normalized(),
        )
        .expect("glob page")
    };
    assert_eq!(page(0)[0].name, "auth.log.1");
    assert_eq!(page(1)[0].name, "auth.log.2");
    assert!(page(2).is_empty(), "only two matching files in /var/log");

    let negated = query_files(
        &index,
        &FileFilter {
            name_glob: Some("auth.log.[^1]".into()),
            path_prefix: Some("/var/log/".into()),
            limit: 10,
            ..FileFilter::default()
        }
        .normalized(),
    )
    .expect("negated class");
    assert_eq!(negated.len(), 1);
    assert_eq!(negated[0].name, "auth.log.2");
}

#[test]
fn glob_uses_whole_names_unicode_characters_and_literal_sql_metacharacters() {
    let (_directory, index) = fixture();
    for (pattern, expected) in [
        ("auth", vec![]),
        ("auth%.log", vec!["auth%.log"]),
        ("auth_.log", vec!["auth_.log"]),
        ("日志?.LOG", vec!["日志甲.LOG"]),
        ("report[[]1].txt", vec!["report[1].txt"]),
        ("auth' OR 1=1 --.log", vec!["auth' OR 1=1 --.log"]),
        ("auth.log.[1-2]", vec!["auth.log.1", "auth.log.2"]),
    ] {
        let found = query_files(
            &index,
            &FileFilter {
                name_glob: Some(pattern.into()),
                limit: 50,
                ..FileFilter::default()
            }
            .normalized(),
        )
        .expect("glob query");
        assert_eq!(
            found
                .iter()
                .map(|hit| hit.name.as_str())
                .collect::<Vec<_>>(),
            expected,
            "pattern: {pattern}"
        );
    }
}
