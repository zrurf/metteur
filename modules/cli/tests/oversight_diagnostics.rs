#[test]
fn report_diagnostics_preserve_categories_and_identify_legacy_absence() {
    let input = serde_json::json!({"reports":[
        {"status":"failed","summary":"Old failure","proposals":[{"state":"rejected"}]},
        {"status":"failed","diagnostic":{"category":"provider_http","stage":"provider","http_status":503,"call_id":"call-1"}},
        {"status":"completed"}
    ]});
    let output = metteur_cli::print::oversight_reports(&input.to_string());
    assert!(output.contains("Unknown legacy"));
    let shown: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(shown["reports"][0]["diagnostic"]["category"], "unknown_legacy");
    assert_eq!(shown["reports"][0]["proposals"][0]["diagnostic"]["category"], "unknown_legacy");
    assert_eq!(shown["reports"][1], input["reports"][1]);
    assert!(shown["reports"][2]["diagnostic"].is_null());
    assert!(input["reports"][0]["diagnostic"].is_null());
}
