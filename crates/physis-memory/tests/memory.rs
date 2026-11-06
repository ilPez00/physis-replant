use physis_memory::{Memory, MemoryNote, MemoryQuery, Observation};
use std::io::Write;
use tempfile::tempdir;

#[test]
fn legacy_observations_and_typed_properties_round_trip() {
    let old = r#"{"seq":1,"at":"2026-11-04T00:00:00Z","source":"fs","subject":"file:a.rs","body":"bytes:1"}"#;
    let row: Observation = serde_json::from_str(old).unwrap();
    assert!(row.props.is_empty());
    assert!(!serde_json::to_string(&row).unwrap().contains("props"));
    let row = row.prop("path", "src/lib.rs").by("watcher").lasting(12);
    assert_eq!(
        serde_json::from_str::<Observation>(&serde_json::to_string(&row).unwrap()).unwrap(),
        row
    );
}

#[test]
fn notes_are_redacted_and_query_is_bounded() {
    let dir = tempdir().unwrap();
    let memory = Memory::new(dir.path().join("notes"));
    for _ in 0..3 {
        memory
            .remember(MemoryNote {
                subject: "audit".into(),
                text: "SERVICE_TOKEN=abcdefgh12345678".into(),
                actor: Some("test".into()),
            })
            .unwrap();
    }
    let page = memory
        .query(&MemoryQuery {
            query: "audit".into(),
            limit: 2,
        })
        .unwrap();
    assert_eq!(page.observations.len(), 2);
    assert!(page.truncated);
    assert!(!page.degraded);
    assert!(!std::fs::read_to_string(memory.path())
        .unwrap()
        .contains("abcdefgh12345678"));
    assert!(memory
        .query(&MemoryQuery {
            query: "".into(),
            limit: 1001
        })
        .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(memory.root())
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
}

#[test]
fn concurrent_single_batch_conditional_and_unique_append_share_one_sequence() {
    let dir = tempdir().unwrap();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let memory = Memory::new(dir.path());
            scope.spawn(move || {
                memory.append(Observation::new("note", "single")).unwrap();
                memory
                    .append_batch([Observation::new("note", "batch")])
                    .unwrap();
                memory
                    .append_if(Observation::new("note", "conditional"), |_| Ok(true))
                    .unwrap();
                memory
                    .append_unique([Observation::new("replant", "same-receipt")])
                    .unwrap();
            });
        }
    });
    let (rows, degraded) = physis_memory::load_observations(&dir.path().join("observations.jsonl"));
    assert!(!degraded);
    assert_eq!(rows.len(), 25);
    assert!(rows.iter().enumerate().all(|(i, row)| row.seq == i as u64));
}

#[test]
fn damaged_unterminated_and_duplicate_sequence_logs_refuse_append_without_data_loss() {
    for tail in [
        "{broken",
        "{broken\n",
        "valid_without_newline",
        "duplicate_sequence",
    ] {
        let dir = tempdir().unwrap();
        let memory = Memory::new(dir.path());
        let row = memory.append(Observation::new("fs", "one")).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(memory.path())
            .unwrap();
        let row_json = serde_json::to_string(&row).unwrap();
        let bytes = match tail {
            "valid_without_newline" => {
                serde_json::to_string(&Observation { seq: 1, ..row }).unwrap()
            }
            "duplicate_sequence" => row_json + "\n",
            _ => tail.into(),
        };
        file.write_all(bytes.as_bytes()).unwrap();
        let original = std::fs::read(memory.path()).unwrap();
        let page = memory.query(&MemoryQuery::default()).unwrap();
        assert!(page.degraded, "{tail}");
        if tail == "duplicate_sequence" {
            assert_eq!(
                page.observations.len(),
                2,
                "retain parsed rows so Next can diagnose the first sequence break"
            );
        }
        assert!(
            memory.append(Observation::new("note", "later")).is_err(),
            "{tail}"
        );
        assert_eq!(std::fs::read(memory.path()).unwrap(), original);
    }
}

#[test]
fn unreadable_log_is_not_empty_and_existing_permissions_are_unchanged() {
    let dir = tempdir().unwrap();
    let memory = Memory::new(dir.path());
    std::fs::create_dir(memory.path()).unwrap();
    assert!(memory.append(Observation::new("note", "x")).is_err());
    assert!(memory.query(&MemoryQuery::default()).unwrap().degraded);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        Memory::new(dir.path())
            .append(Observation::new("note", "x"))
            .unwrap();
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
