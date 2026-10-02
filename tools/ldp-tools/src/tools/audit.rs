//! `ldp-audit` — the hash-chained audit-log reader / verifier.
//!
//! ```text
//! ldp-audit <FILE> [--records] [--expect-head HEX]
//! ldp-audit --live [--socket NAME]
//! ```
//!
//! The offline half is [`crate::audit_log::load`] surfaced for
//! operators: parse the JSONL persistence of the Phase 16 audit
//! ring, rebuild the chain, recompute every digest, and report the
//! verdict. `--records` prints one line per record (who did what,
//! when, under which scope); `--expect-head HEX` compares the head
//! digest against an operator-held checkpoint — the *only* defense
//! against a truncated or wholly rewritten tail, which SHA-256
//! cannot detect from the file alone.
//!
//! The `--live` half follows the Phase 18 doctrine for offline-core
//! tools: connect, bootstrap, and report what the running server
//! actually offers — the `ldp.security.security` global is the
//! interface whose broker writes the log this tool reads, so its
//! absence is reported honestly (the Phase 10 vertical slice does
//! not serve it) instead of fabricated.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;

use ldp_security::chain::AuditAction;

use crate::args::{Args, UsageError};
use crate::audit_log;
use crate::session::ToolSession;
use crate::socket;

/// The security interface whose broker persists the audit chain.
const SECURITY_GLOBAL: &str = "ldp.security.security";

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct AuditArgs {
    /// The log file to verify (positional).
    pub file: Option<PathBuf>,
    /// `--records`: print one line per record.
    pub records: bool,
    /// `--expect-head HEX`: the operator-held checkpoint (64 hex).
    pub expect_head: Option<String>,
    /// `--live`: inspect a running server instead.
    pub live: bool,
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown arguments, missing values, or a file
/// given together with `--live`.
pub fn parse(mut argv: Args) -> Result<AuditArgs, UsageError> {
    let records = argv.take_flag("--records");
    let mut expect_head = None;
    if let Some(head) = argv.take_value("--expect-head")? {
        if head.len() != 64 || !head.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(UsageError(format!(
                "--expect-head wants 64 hex characters, got {head:?}"
            )));
        }
        expect_head = Some(head.to_ascii_lowercase());
    }
    let socket = argv.take_value("--socket")?;
    let live = argv.take_flag("--live");
    let file = argv.take_positional().map(PathBuf::from);
    argv.require_empty()?;
    if live && file.is_some() {
        return Err(UsageError(
            "a log file cannot be combined with --live".to_owned(),
        ));
    }
    if !live && file.is_none() {
        return Err(UsageError(
            "give a log file (or --live to inspect a running server)".to_owned(),
        ));
    }
    Ok(AuditArgs {
        file,
        records,
        expect_head,
        live,
        socket,
    })
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-audit — hash-chained audit-log reader / verifier\n\
     \n\
     USAGE:\n\
     \x20 ldp-audit <FILE> [--records] [--expect-head HEX]\n\
     \x20 ldp-audit --live [--socket NAME]\n\
     \n\
     OPTIONS:\n\
     \x20 <FILE>            the JSONL audit log to verify\n\
     \x20 --records         print one line per record\n\
     \x20 --expect-head HEX operator checkpoint (catches truncated tails)\n\
     \x20 --live            inspect what a running server offers\n\
     \x20 --socket NAME     abstract socket name (or LDP_SOCKET)\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; verification failures print and return 1.
///
/// # Panics
/// When `--live` is absent but no file was given — [`parse`] rejects
/// that combination first, so the invariant holds by construction.
pub fn run(args: &AuditArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    if args.live {
        return Ok(live_report(args, out));
    }
    let path = args
        .file
        .as_ref()
        .expect("parse guarantees a file without --live");
    let loaded = match audit_log::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            let _ = writeln!(out, "ldp-audit: {e}");
            return Ok(1);
        }
    };
    let _ = writeln!(
        out,
        "audit: {} record(s) loaded from {}",
        loaded.records,
        path.display()
    );
    if args.records {
        for record in loaded.chain.records() {
            let _ = writeln!(out, "  {}", format_record(record));
        }
    }
    let _ = writeln!(out, "audit: head {}", loaded.head_hex());
    let mut failed = false;
    if loaded.verdict.ok {
        let _ = writeln!(
            out,
            "audit: chain verified ({} record(s) recomputed)",
            loaded.verdict.records
        );
    } else {
        failed = true;
        let _ = writeln!(
            out,
            "audit: CHAIN BROKEN — the log was edited, reordered, or \
             corrupted after the fact ({} record(s) covered)",
            loaded.verdict.records
        );
    }
    if let Some(expected) = &args.expect_head {
        if *expected == loaded.head_hex() {
            let _ = writeln!(out, "audit: head matches the operator checkpoint");
        } else {
            failed = true;
            let _ = writeln!(
                out,
                "audit: HEAD MISMATCH — expected {expected}, found {} \
                 (a truncated or rewritten tail is consistent by \
                 construction; only the checkpoint catches it)",
                loaded.head_hex()
            );
        }
    }
    Ok(i32::from(failed))
}

/// The `--live` registry report.
fn live_report(args: &AuditArgs, out: &mut dyn Write) -> i32 {
    let addr = match socket::resolve(
        args.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    ) {
        Ok(addr) => addr,
        Err(e) => {
            let _ = writeln!(out, "ldp-audit: {e}");
            return 1;
        }
    };
    let mut session = match ToolSession::connect(&addr) {
        Ok(session) => session,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-audit: connect {} failed: {e}",
                addr.display_string()
            );
            return 1;
        }
    };
    if let Err(e) = session.bootstrap() {
        let _ = writeln!(out, "ldp-audit: bootstrap failed: {e}");
        return 1;
    }
    let _ = writeln!(
        out,
        "live: {} ({} global(s) advertised)",
        addr.display_string(),
        session.globals().len()
    );
    for global in session.globals() {
        let _ = writeln!(
            out,
            "  {} v{}-v{}",
            global.interface, global.version_min, global.version_max
        );
    }
    if session.has_global(SECURITY_GLOBAL) {
        let _ = writeln!(
            out,
            "live: {SECURITY_GLOBAL} is served — the audit broker is \
             live on this server; point ldp-audit at the log it \
             persists (the deployment's $XDG_STATE_HOME/ldp/audit.jsonl)"
        );
    } else {
        let _ = writeln!(
            out,
            "live: {SECURITY_GLOBAL} is NOT served — this server runs \
             no audit broker, so no audit log exists to verify; the \
             offline half of this tool (ldp-audit <FILE>) applies to \
             any log produced by a Phase 16 deployment"
        );
    }
    0
}

/// One record as one operator-readable line.
fn format_record(record: &ldp_security::chain::ChainedRecord) -> String {
    let event = &record.event;
    let scope = event.scope.map_or_else(|| "-".to_owned(), scope_name);
    let digest = &ldp_security::sha256::hex32(&record.digest);
    format!(
        "seq {:>4}  ts {:>12}  client {:>3}  {:<24}  {:<8}  {:<18}  {}  digest {}…",
        record.seq,
        event.ts_ns,
        event.client,
        truncate(&event.app_id, 24),
        action_name(event.action),
        scope,
        truncate(&event.detail, 40),
        &digest[..8]
    )
}

/// The canonical scope names (`ldp_core::caps::Scope`).
fn scope_name(scope: ldp_core::caps::Scope) -> String {
    use ldp_core::caps::Scope;
    let name = match scope {
        Scope::Screenshot => "Screenshot",
        Scope::ScreenRecord => "ScreenRecord",
        Scope::InputInject => "InputInject",
        Scope::GlobalShortcut => "GlobalShortcut",
        Scope::ClipboardRead => "ClipboardRead",
        Scope::InputGrab => "InputGrab",
        Scope::ConfigureDisplay => "ConfigureDisplay",
        Scope::ManageWorkspaces => "ManageWorkspaces",
        Scope::A11yControl => "A11yControl",
        Scope::AuditRead => "AuditRead",
        Scope::Bridge => "Bridge",
        Scope::ProtectedSurface => "ProtectedSurface",
        _ => "(unknown)",
    };
    name.to_owned()
}

/// The canonical action names.
const fn action_name(action: AuditAction) -> &'static str {
    match action {
        AuditAction::Grant => "grant",
        AuditAction::Deny => "deny",
        AuditAction::Revoke => "revoke",
        AuditAction::Capture => "capture",
        AuditAction::Inject => "inject",
        AuditAction::Bridge => "bridge",
        _ => "(unknown)",
    }
}

/// `s` cut to `width` with an ellipsis when longer.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(width.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Args;
    use crate::audit_log::ChainLog;
    use ldp_core::caps::Scope;
    use ldp_security::chain::{AuditAction, AuditEvent};

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ldp-tools-audit-tool-{tag}-{}-{}",
            std::process::id(),
            line!()
        ))
    }

    fn event(action: AuditAction, client: u32, ts: u64) -> AuditEvent {
        AuditEvent {
            ts_ns: ts,
            client,
            app_id: "org.example.app".to_owned(),
            scope: Some(Scope::ClipboardRead),
            action,
            detail: "{\"prompt\":\"granted\"}".to_owned(),
        }
    }

    fn write_log(path: &std::path::Path, n: u64) {
        let mut log = ChainLog::create(path).unwrap();
        for i in 0..n {
            log.append(event(AuditAction::Grant, 7, 1000 * (i + 1)))
                .unwrap();
        }
    }

    #[test]
    fn parse_accepts_file_and_options() {
        let args = parse(Args::from_argv([
            "audit.jsonl",
            "--records",
            "--expect-head",
            &"a".repeat(64),
        ]))
        .unwrap();
        assert_eq!(
            args.file.map(|p| p.display().to_string()),
            Some("audit.jsonl".to_owned())
        );
        assert!(args.records);
        assert_eq!(args.expect_head.as_deref(), Some(&"a".repeat(64)[..]));
    }

    #[test]
    fn parse_rejects_bad_head_and_conflicting_modes() {
        assert!(parse(Args::from_argv(["f.jsonl", "--expect-head", "zz"])).is_err());
        assert!(parse(Args::from_argv(["f.jsonl", "--live"])).is_err());
        assert!(parse(Args::from_argv([""; 0])).is_err());
        assert!(parse(Args::from_argv(["--live"])).is_ok());
    }

    #[test]
    fn verified_log_exits_zero_and_reports_head() {
        let path = temp_path("ok");
        write_log(&path, 12);
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([path.display().to_string()])).unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("chain verified"));
        assert!(text.contains("audit: head "));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn records_listing_names_every_field() {
        let path = temp_path("list");
        write_log(&path, 2);
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([
                path.display().to_string(),
                "--records".to_owned(),
            ]))
            .unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("org.example.app"));
        assert!(text.contains("grant"));
        assert!(text.contains("ClipboardRead"));
        assert!(text.contains("seq    1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tampered_chain_exits_one() {
        let path = temp_path("tampered");
        write_log(&path, 10);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("\"client\":7", "\"client\":9")).unwrap();
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([path.display().to_string()])).unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 1);
        assert!(String::from_utf8(out).unwrap().contains("CHAIN BROKEN"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truncated_tail_is_caught_by_the_checkpoint_only() {
        let path = temp_path("truncated");
        write_log(&path, 10);
        let head = {
            let mut log_probe = audit_log::load(&path).unwrap();
            let head = log_probe.head_hex();
            let _ = &mut log_probe;
            head
        };
        let text = std::fs::read_to_string(&path).unwrap();
        let truncated: String = text.lines().take(4).fold(String::new(), |mut acc, l| {
            acc.push_str(l);
            acc.push('\n');
            acc
        });
        std::fs::write(&path, truncated).unwrap();
        // Without a checkpoint: internally consistent, exit 0 …
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([path.display().to_string()])).unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        // … with it: exit 1 naming the mismatch.
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([
                path.display().to_string(),
                "--expect-head".to_owned(),
                head.clone(),
            ]))
            .unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 1);
        assert!(String::from_utf8(out).unwrap().contains("HEAD MISMATCH"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_file_exits_one_with_the_line_number() {
        let path = temp_path("malformed");
        std::fs::write(&path, "not jsonl at all\n").unwrap();
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([path.display().to_string()])).unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 1);
        assert!(String::from_utf8(out).unwrap().contains("line 1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_log_is_a_verified_empty_chain() {
        let path = temp_path("empty");
        std::fs::write(&path, "").unwrap();
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([path.display().to_string()])).unwrap(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("0 record(s)"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truncation_ellipsizes_long_values() {
        assert_eq!(truncate("short", 10), "short");
        let long = "x".repeat(41);
        let cut = truncate(&long, 40);
        assert_eq!(cut.chars().count(), 40);
        assert!(cut.ends_with('…'));
        assert!(cut.starts_with('x'));
    }

    #[test]
    fn scope_and_action_names_are_canonical() {
        assert_eq!(scope_name(Scope::AuditRead), "AuditRead");
        assert_eq!(action_name(AuditAction::Bridge), "bridge");
    }
}
