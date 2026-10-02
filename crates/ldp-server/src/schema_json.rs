//! Compact JSON rendering of the compiled schema — the payload of
//! `registry.schema` events.
//!
//! The rendering is deterministic (table order, compact separators) and
//! deliberately excludes documentation strings: machines consume it, the
//! reference markdown documents it, and keeping every interface's payload
//! under the 4096-byte default string limit is a wire constraint
//! (`Limits::string_bytes`). The [`sizes_fit`] conformance check below
//! pins that property for the whole v1 surface.

use std::fmt::Write as _;

use ldp_protocol::schema::{ArgSchema, InterfaceSchema, ModuleSchema, OpSchema};

/// Every interface of the compiled protocol, in module then declaration
/// order — the order empty-interface `introspect` replies stream in.
pub fn all_interfaces() -> impl Iterator<Item = (&'static ModuleSchema, &'static InterfaceSchema)> {
    ldp_protocol::MODULES
        .iter()
        .flat_map(|&m| m.interfaces.iter().map(move |&i| (m, i)))
}

/// Render one interface's schema as compact JSON.
#[must_use]
pub fn render_interface(module: &ModuleSchema, iface: &InterfaceSchema) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("{\"name\":");
    escape(&mut out, iface.name);
    out.push_str(",\"module\":");
    escape(&mut out, module.name);
    out.push_str(",\"version_min\":");
    push_u32(&mut out, iface.version_min);
    out.push_str(",\"version_max\":");
    push_u32(&mut out, iface.version_max);
    out.push_str(",\"global\":");
    out.push_str(if iface.global { "true" } else { "false" });
    if let Some(id) = iface.builtin_id {
        out.push_str(",\"builtin_id\":");
        push_u32(&mut out, id);
    }
    out.push_str(",\"requests\":[");
    render_ops(&mut out, iface.requests);
    out.push_str("],\"events\":[");
    render_ops(&mut out, iface.events);
    out.push_str("]}");
    out
}

fn render_ops(out: &mut String, ops: &'static [OpSchema]) {
    for (i, op) in ops.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        escape(out, op.name);
        out.push_str(",\"opcode\":");
        push_u32(out, op.opcode);
        out.push_str(",\"since\":");
        push_u32(out, op.since);
        if let Some(reply) = op.reply {
            out.push_str(",\"reply\":");
            escape(out, reply);
        }
        out.push_str(",\"args\":[");
        for (j, arg) in op.args.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            render_arg(out, arg);
        }
        out.push_str("]}");
    }
}

fn render_arg(out: &mut String, arg: &ArgSchema) {
    out.push_str("{\"name\":");
    escape(out, arg.name);
    out.push_str(",\"ty\":");
    escape(out, arg.ty.as_str());
    if let Some(of) = arg.of {
        out.push_str(",\"of\":");
        escape(out, of);
    }
    if arg.nullable {
        out.push_str(",\"nullable\":true");
    }
    out.push('}');
}

/// JSON string escaping for the (identifier-shaped) schema content. The
/// schema never contains control characters or quotes today; the escaper
/// exists so a future spec change cannot produce invalid JSON silently.
fn escape(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn push_u32(out: &mut String, v: u32) {
    out.push_str(&v.to_string());
}

/// Whether every interface's rendering fits `string_bytes` — the
/// conformance property that makes `registry.schema` deliverable under
/// default limits.
#[must_use]
pub fn sizes_fit(string_bytes: u32) -> bool {
    all_interfaces().enumerate().all(|(i, (m, iface))| {
        let size = render_interface(m, iface).len();
        if size > string_bytes as usize {
            // Fail with the first offender identified.
            debug_assert!(false, "interface {i} ({}) renders {size} bytes", iface.name);
            return false;
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interface_fits_the_default_string_limit() {
        assert!(sizes_fit(ldp_core::limits::Limits::DEFAULT.string_bytes));
    }

    #[test]
    fn rendering_is_deterministic_and_parseable_shape() {
        let (module, iface) = all_interfaces()
            .find(|(_, i)| i.name == "ldp.core.connection")
            .unwrap();
        let json = render_interface(module, iface);
        // Deterministic: same input, same output.
        assert_eq!(json, render_interface(module, iface));
        // Shape: the five connection requests appear in opcode order,
        // including the Phase 4 amendment.
        for needle in [
            "\"name\":\"ldp.core.connection\"",
            "\"builtin_id\":1",
            "\"name\":\"hello\",\"opcode\":1",
            "\"name\":\"sync\",\"opcode\":2",
            "\"name\":\"destroy\",\"opcode\":3",
            "\"name\":\"ping\",\"opcode\":4",
            "\"name\":\"get_registry\",\"opcode\":5",
            "\"reply\":\"welcome\"",
            "\"ty\":\"bitset\",\"of\":\"connection_options\"",
        ] {
            assert!(json.contains(needle), "missing {needle} in: {json}");
        }
        // The error event carries the enum-of.
        assert!(json.contains("\"name\":\"error\",\"opcode\":5"));
        assert!(json.contains("\"ty\":\"enum\",\"of\":\"error_code\""));
    }

    #[test]
    fn all_interfaces_covers_the_whole_surface() {
        // 33 v1 interfaces + capture_manager (Phase 22).
        assert_eq!(all_interfaces().count(), 34);
    }

    #[test]
    fn escaping_handles_specials() {
        let mut s = String::new();
        escape(&mut s, "a\"b\\c\n");
        assert_eq!(s, "\"a\\\"b\\\\c\\n\"");
    }
}
