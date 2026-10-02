//! `ldp-validate` — spec-set compilation and live schema cross-check.
//!
//! ```text
//! ldp-validate [--spec-dir DIR] [--live [--socket NAME]]
//! ```
//!
//! The offline half is `ldpc::compile_dir` surfaced for operators:
//! every diagnostic prints as `file: path: message` and the run exits
//! 1 on any of them (the same validation the protocol compiler runs,
//! no python required). The `--live` half connects to a running
//! server, pulls one introspection payload per *advertised* global,
//! and cross-checks each served interface against the compiled
//! registry — a deployment whose served schema has drifted from the
//! build it runs with is a deployment worth failing.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;

use ldp_protocol::REGISTRY;

use crate::args::{Args, UsageError};
use crate::session::ToolSession;
use crate::socket;

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct ValidateArgs {
    /// `--spec-dir DIR` (default `spec`).
    pub spec_dir: Option<PathBuf>,
    /// `--live`: cross-check a running server.
    pub live: bool,
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown arguments or missing values.
pub fn parse(mut argv: Args) -> Result<ValidateArgs, UsageError> {
    let spec_dir = argv.take_value("--spec-dir")?.map(PathBuf::from);
    let socket = argv.take_value("--socket")?;
    let live = argv.take_flag("--live");
    argv.require_empty()?;
    Ok(ValidateArgs {
        spec_dir,
        live,
        socket,
    })
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-validate — spec set + live schema validation\n\
     \n\
     USAGE:\n\
     \x20 ldp-validate [--spec-dir DIR] [--live [--socket NAME]]\n\
     \n\
     OPTIONS:\n\
     \x20 --spec-dir DIR     spec module directory (default: spec)\n\
     \x20 --live             cross-check the served schema of a running server\n\
     \x20 --socket NAME      abstract socket name (or LDP_SOCKET)\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; validation failures print and return 1.
pub fn run(args: &ValidateArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    let mut failures = 0;
    failures += spec_report(args, out) as i32;
    if args.live {
        failures += live_report(args, out) as i32;
    }
    Ok(i32::from(failures > 0))
}

/// Compile the spec set and report the verdict.
fn spec_report(args: &ValidateArgs, out: &mut dyn Write) -> bool {
    let dir = args
        .spec_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("spec"));
    let _ = writeln!(out, "spec: compiling {} …", dir.display());
    match ldpc::compile_dir(&dir) {
        Ok(set) => {
            let counts = set.counts();
            let _ = writeln!(
                out,
                "spec OK: {} module(s), {} interface(s), {} request(s), \
                 {} event(s), {} enum(s), {} bitset(s)",
                counts.modules,
                counts.interfaces,
                counts.requests,
                counts.events,
                counts.enums,
                counts.bitsets
            );
            false
        }
        Err(diags) => {
            let _ = writeln!(out, "spec FAILED: {} diagnostic(s)", diags.len());
            for diag in &diags {
                let _ = writeln!(out, "  {diag}");
            }
            true
        }
    }
}

/// Cross-check the served schema of a running server.
fn live_report(args: &ValidateArgs, out: &mut dyn Write) -> bool {
    let addr = match socket::resolve(
        args.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    ) {
        Ok(addr) => addr,
        Err(e) => {
            let _ = writeln!(out, "ldp-validate: {e}");
            return true;
        }
    };
    let mut session = match ToolSession::connect(&addr) {
        Ok(session) => session,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-validate: connect {} failed: {e}",
                addr.display_string()
            );
            return true;
        }
    };
    if let Err(e) = session.bootstrap() {
        let _ = writeln!(out, "ldp-validate: bootstrap failed: {e}");
        return true;
    }
    let _ = writeln!(
        out,
        "live: {} ({} global(s) advertised)",
        addr.display_string(),
        session.globals().len()
    );

    let mut failures = 0;
    for global in session.globals().to_vec() {
        match session.introspect(&global.interface) {
            Ok(payloads) => {
                for (name, json) in payloads {
                    match served_matches_compiled(&name, &json) {
                        Ok(()) => {
                            let _ = writeln!(out, "  {name}: schema matches the compiled registry");
                        }
                        Err(problem) => {
                            failures += 1;
                            let _ = writeln!(out, "  {name}: {problem}");
                        }
                    }
                }
            }
            Err(e) => {
                failures += 1;
                let _ = writeln!(out, "  {}: introspection failed: {e}", global.interface);
            }
        }
    }
    if failures == 0 {
        let _ = writeln!(out, "live: served schema matches the compiled registry");
        false
    } else {
        let _ = writeln!(out, "live: {failures} mismatch(es)");
        true
    }
}

/// Compare one served introspection payload with the compiled schema.
fn served_matches_compiled(name: &str, json: &str) -> Result<(), String> {
    let compiled = REGISTRY
        .modules()
        .iter()
        .flat_map(|m| m.interfaces.iter())
        .find(|i| i.name == name)
        .ok_or_else(|| format!("served interface '{name}' is unknown to this build"))?;
    let served_name = json_string_field(json, "name")
        .ok_or_else(|| "payload has no parsable \"name\" field".to_string())?;
    if served_name != name {
        return Err(format!(
            "payload name \"{served_name}\" disagrees with the event's interface \"{name}\""
        ));
    }
    let version_min = json_number_field(json, "version_min")
        .ok_or_else(|| "payload has no parsable \"version_min\"".to_owned())?;
    let version_max = json_number_field(json, "version_max")
        .ok_or_else(|| "payload has no parsable \"version_max\"".to_owned())?;
    if version_min != u64::from(compiled.version_min)
        || version_max != u64::from(compiled.version_max)
    {
        return Err(format!(
            "served v{version_min}-v{version_max} vs compiled v{}-v{}",
            compiled.version_min, compiled.version_max
        ));
    }
    let served_requests = json_array_len(json, "requests");
    let served_events = json_array_len(json, "events");
    if served_requests != Some(compiled.requests.len())
        || served_events != Some(compiled.events.len())
    {
        return Err(format!(
            "served op counts ({:?} requests, {:?} events) vs compiled \
             ({} requests, {} events)",
            served_requests,
            served_events,
            compiled.requests.len(),
            compiled.events.len()
        ));
    }
    Ok(())
}

/// Extract `"field":"value"` from a compact JSON object (top level).
fn json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let mut end = None;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let end = end?;
    Some(rest[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Extract `"field":N` from a compact JSON object (top level).
fn json_number_field(json: &str, field: &str) -> Option<u64> {
    let needle = format!("\"{field}\":");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Count the elements of `"field":[…]` (top level).
fn json_array_len(json: &str, field: &str) -> Option<usize> {
    let needle = format!("\"{field}\":[");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    if rest.starts_with(']') {
        return Some(0);
    }
    // Depth scan: elements at depth 0 (relative to the array).
    let mut depth = 0usize;
    let mut elements = 1usize;
    let mut in_string = false;
    let mut escaped = false;
    for c in rest.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                if depth == 0 {
                    return Some(elements);
                }
                depth -= 1;
            }
            ',' if depth == 0 => elements += 1,
            _ => {}
        }
    }
    Some(elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "{\"name\":\"ldp.core.shm\",\"module\":\"ldp.core\",\
        \"version_min\":1,\"version_max\":1,\"global\":true,\
        \"requests\":[{\"name\":\"create_pool\"},{\"name\":\"x\"}],\"events\":[]}";

    #[test]
    fn json_fields_are_extracted() {
        assert_eq!(
            json_string_field(PAYLOAD, "name").as_deref(),
            Some("ldp.core.shm")
        );
        assert_eq!(json_number_field(PAYLOAD, "version_min"), Some(1));
        assert_eq!(json_array_len(PAYLOAD, "requests"), Some(2));
        assert_eq!(json_array_len(PAYLOAD, "events"), Some(0));
    }

    #[test]
    fn missing_fields_are_none() {
        assert_eq!(json_string_field(PAYLOAD, "nonesuch"), None);
        assert_eq!(json_number_field("{}", "version_min"), None);
    }

    #[test]
    fn nested_arrays_count_top_level_only() {
        let nested = "{\"ops\":[{\"args\":[1,2,3]},{\"args\":[]}]}";
        assert_eq!(json_array_len(nested, "ops"), Some(2));
    }

    #[test]
    fn string_with_brackets_is_transparent() {
        let doc = "{\"ops\":[{\"name\":\"a]b\"}]}";
        assert_eq!(json_array_len(doc, "ops"), Some(1));
    }
}
