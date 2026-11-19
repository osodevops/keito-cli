//! Human-readable output compatibility controls captured before tabled migration.
#[test]
fn rounded_tables_preserve_headers_values_spacing_and_empty_output() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/table-output-controls.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let actual = keito_cli::output::table::to_table(case["items"].as_array().unwrap());
        assert_eq!(
            actual,
            case["expected"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
}
