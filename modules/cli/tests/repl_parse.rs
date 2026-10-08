//! Parsing tests for the CLI REPL command language.

use metteur_cli::commands::{Command, parse};

#[test]
fn compile_distinguishes_print_save_and_explicit_identity() {
    for (suffix, save, id) in [("", false, None), (" save", true, None), (" save as target", true, Some("target"))] {
        assert_eq!(parse(&format!("bp compile plan.mbp{suffix}")), Ok(Command::BpCompile {
            file: "plan.mbp".into(), save, save_to: id.map(str::to_owned),
        }));
    }
    assert!(parse("bp compile plan.mbp save as").is_err());
}

#[test]
fn parses_workspace_commands() {
    assert_eq!(
        parse("open C:/repo"),
        Ok(Command::Open {
            path: "C:/repo".into()
        })
    );
    assert_eq!(
        parse("close  /tmp/w"),
        Ok(Command::Close {
            path: "/tmp/w".into()
        })
    );
    assert_eq!(parse("ws"), Ok(Command::Ws));
    assert!(parse("open").is_err());
    assert!(parse("open a b").is_err());
}

#[test]
fn parses_blueprint_commands() {
    assert_eq!(
        parse("save-bp bp.json"),
        Ok(Command::SaveBp {
            file: "bp.json".into(),
            id: None
        })
    );
    assert_eq!(
        parse("save-bp bp.json 11111111-1111-1111-1111-111111111111"),
        Ok(Command::SaveBp {
            file: "bp.json".into(),
            id: Some("11111111-1111-1111-1111-111111111111".into()),
        })
    );
    assert_eq!(
        parse("load-bp abc out.json"),
        Ok(Command::LoadBp {
            id: "abc".into(),
            file: Some("out.json".into())
        })
    );
    assert_eq!(
        parse("exec bp-id"),
        Ok(Command::Exec {
            blueprint_id: "bp-id".into()
        })
    );
    assert_eq!(
        parse("cont run-1"),
        Ok(Command::Cont {
            run_id: "run-1".into()
        })
    );
    assert_eq!(parse("runs"), Ok(Command::Runs));
}

#[test]
fn parses_control_and_interrupts() {
    assert_eq!(parse("cancel"), Ok(Command::Cancel));
    assert_eq!(parse("pause"), Ok(Command::Pause));
    assert_eq!(parse("resume"), Ok(Command::Resume));
    assert_eq!(
        parse("say urgent stop now"),
        Ok(Command::Say {
            priority: "Urgent".into(),
            message: "stop now".into()
        })
    );
    // Invalid priorities are rejected.
    assert!(parse("say asap stop").is_err());
    assert!(parse("say urgent").is_err());
}

#[test]
fn parses_approval_commands() {
    assert_eq!(
        parse("approve req-1 AllowOnce"),
        Ok(Command::Approve {
            request_id: "req-1".into(),
            decision: "AllowOnce".into()
        })
    );
    // Shorthand forms expand to canonical decisions.
    assert_eq!(
        parse("approve req-1 deny workspace"),
        Ok(Command::Approve {
            request_id: "req-1".into(),
            decision: "DenyWorkspace".into()
        })
    );
    assert!(parse("approve req-1 Sometimes").is_err());
    assert_eq!(parse("approve-auto on"), Ok(Command::ApproveAuto(true)));
    assert_eq!(parse("approve-auto off"), Ok(Command::ApproveAuto(false)));
    assert!(parse("approve-auto maybe").is_err());
}

#[test]
fn parses_registry_versioning_config_and_misc() {
    assert_eq!(parse("tools"), Ok(Command::Tools));
    assert_eq!(parse("nodes"), Ok(Command::Nodes));
    assert_eq!(
        parse("snap release candidate"),
        Ok(Command::Snap {
            description: "release candidate".into(),
            alias: None
        })
    );
    assert_eq!(parse("snaps"), Ok(Command::Snaps));
    assert_eq!(
        parse("rollback snap-9"),
        Ok(Command::Rollback {
            target: "snap-9".into()
        })
    );
    assert_eq!(
        parse("hist src/main.rs"),
        Ok(Command::Hist {
            path: "src/main.rs".into()
        })
    );
    assert_eq!(parse("audit ws"), Ok(Command::AuditWs));
    assert_eq!(parse("audit global"), Ok(Command::AuditGlobal));
    assert!(parse("audit both").is_err());
    assert_eq!(
        parse("cfg get"),
        Ok(Command::CfgGet {
            workspace: false
        })
    );
    assert_eq!(
        parse("cfg get ws"),
        Ok(Command::CfgGet {
            workspace: true
        })
    );
    assert_eq!(
        parse(r#"cfg set {"a":1} ws"#),
        Ok(Command::CfgSet {
            json: r#"{"a":1}"#.into(),
            workspace: true
        })
    );
    assert_eq!(
        parse("usage run-7"),
        Ok(Command::Usage {
            run_id: "run-7".into()
        })
    );
    assert_eq!(parse("mcp"), Ok(Command::Mcp));
    assert_eq!(parse("status"), Ok(Command::Status));
    assert_eq!(parse("help"), Ok(Command::Help));
    assert_eq!(parse("exit"), Ok(Command::Exit));
    assert!(parse("nonsense").is_err());
    assert!(parse("").is_err());
}
