//! The former Python integration wrapper is preserved in the dated evidence
//! archive. Native installer/mapping acceptance lives in the Rust module tests;
//! process/ACK/deadline acceptance lives in native_capture_guard.rs.
#[test]
fn active_capture_bundle_contains_no_python_or_development_test_payload() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/keito-time-track");
    fn visit(path: &std::path::Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            assert_ne!(path.extension().and_then(|s| s.to_str()), Some("py"));
            assert_ne!(path.file_name().and_then(|s| s.to_str()), Some("tests"));
            if path.is_dir() {
                visit(&path);
            }
        }
    }
    visit(&root);
    let dispatch = std::fs::read_to_string(root.join("hooks/dispatch.sh")).unwrap();
    assert!(dispatch.contains("KEITO_CAPTURE_GUARD_BIN"));
    assert!(!dispatch.contains("python"));
}
