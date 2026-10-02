//! The compositor dispatcher: every window-system decision on the
//! protocol's request path.
//!
//! One [`CompositorDispatcher`] per session thread, all sharing the
//! [`World`] through its one lock. Requests route
//! by `(interface, operation)`; replies and spontaneous events emit
//! through [`DispatchCtx`] directly, while anything another client must
//! hear parks in the outboxes (the frame loop routes by ownership).
//!
//! The handler families:
//!
//! * `ldp.core.compositor` — surface/subsurface factories,
//! * `ldp.core.surface` — the pending-state setters, `attach`,
//!   `damage[_buffer]`, `commit`, `frame`,
//! * `ldp.core.subsurface` — position/stacking/mode,
//! * `ldp.core.shm` / `ldp.core.shm_pool` — pool and buffer exchange,
//! * `ldp.core.output` — bind-time cascade only (no requests),
//! * `ldp.capture.capture_manager` — output frame snapshots (the
//!   Phase 22 capture surface: `grab` → a read-once memfd `frame`).
//!
//! Argument shapes are guaranteed by stage-3 validation; the handlers
//! re-check the *value* domains (enum ranges, geometry) and fail with
//! the matching taxonomy code, which the session core turns into the
//! fatal `connection.error` — the only rejection channel v1 has.

#![forbid(unsafe_code)]

use std::sync::Arc;

use ldp_clipboard::{
    AdmissionError, DataEvent, DeviceError, DndError, FinishError, OfferError, ReceiveError,
    Routed, Slot, SourceError,
};
use ldp_compositor::surface::{SubsurfaceMode, Surface};
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::color::{
    ColorDescription, ColorRange, HdrMetadata, Luminance, Primaries, TransferFunction,
};
use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_core::ids::{ClientId, ObjectId};
use ldp_core::scale::ScaleFactor;
use ldp_core::time::PresentationMode;
use ldp_core::wire::{ArgType, Value};
use ldp_server::{DispatchCtx, Dispatcher, IncomingRequest, ObjectEntry, SessionEnd};
use ldp_transport::FdList;

use crate::outbox::OutboxEntry;
use crate::scene::{ObjectKey, Shared, World};
use crate::shm::{ObjectKeyRef, ShmBuffer, ShmError, ShmPool, SHM_FORMATS};

/// One toplevel state verb, decoded (Phase 49, the states arm): the
/// request's name and argument resolved into the machine's intent —
/// the driver's single vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToplevelIntent {
    /// `maximize` — fill the workspace area minus decorations.
    Maximize,
    /// `unmaximize` — release the workspace fill.
    Unmaximize,
    /// `fullscreen(output|null)` — cover the output; the argument is
    /// the client's bound output object (`None`: the current one).
    Fullscreen(Option<ObjectId>),
    /// `unfullscreen` — release the output cover.
    Unfullscreen,
    /// `minimize` — hidden from the space, kept alive.
    Minimize,
    /// `unminimize` — the shell decides when to restore (the owner's
    /// own ask always serves).
    Unminimize,
    /// `set_sticky` — show on all spaces.
    Sticky(bool),
    /// `set_workspace` — move to the space (clamped; the actual rides
    /// `workspace_changed`).
    Workspace(u32),
}

/// The per-session window-system dispatcher.
pub struct CompositorDispatcher {
    shared: Arc<Shared>,
}

impl CompositorDispatcher {
    /// A dispatcher over the shared world.
    #[must_use]
    pub fn new(shared: Arc<Shared>) -> CompositorDispatcher {
        CompositorDispatcher { shared }
    }

    /// Emit a batch of entries through this session (liveness-checked;
    /// dead targets drop with their descriptors).
    fn emit_entries(ctx: &mut DispatchCtx<'_>, entries: Vec<OutboxEntry>) -> Result<()> {
        for e in entries {
            if let Some(reason) = e.revoke {
                // A revocation the frame loop queued (the output object
                // died with its pipeline). The store check is the same
                // liveness gate: an object the client already destroyed
                // (or a session racing its own teardown) skips the
                // revoke instead of erroring on it.
                if matches!(
                    ctx.store().lookup(e.target),
                    ldp_server::object::Lookup::Live(_)
                ) {
                    ctx.revoke(e.target, reason)?;
                }
                continue;
            }
            if !matches!(
                ctx.store().lookup(e.target),
                ldp_server::object::Lookup::Live(_)
            ) {
                continue;
            }
            // A server-chosen object the event references (the data
            // family's `data_offer` announcement): the receiver must
            // hold it before the event makes sense. The id is
            // world-unique, so an error here means the client raced
            // its own teardown or hit the object ceiling — the entry
            // drops with the rest rather than failing the session.
            if let Some((id, interface)) = e.create {
                if ctx.create_announced(id, interface, 1).is_err() {
                    continue;
                }
            }
            if let Some(fd) = e.fd {
                let mut fds = FdList::new();
                fds.push(fd);
                ctx.emit_fd(e.target, e.event, e.args, &mut fds)?;
            } else {
                ctx.emit(e.target, e.event, e.args)?;
            }
        }
        Ok(())
    }

    /// `compositor.create_surface`.
    /// The seat's device proxies (Phase 31's mint surface, Phase 33's
    /// routing): the five `get_*` requests create the per-device
    /// objects input events target *and* record the client's binding
    /// (what the router addresses). A keyboard mint carries the
    /// keymap descriptor and the repeat model on the same dispatch —
    /// the one-time-per-keyboard contract.
    fn seat_request(&self, ctx: &mut DispatchCtx<'_>, request: &IncomingRequest<'_>) -> Result<()> {
        let Value::NewId(id) = request.args[0] else {
            return Err(bad_shape("seat.get_device"));
        };
        let interface = match request.op.name {
            "get_keyboard" => "ldp.input.keyboard",
            "get_touch" => "ldp.input.touch",
            "get_tablet" => "ldp.input.tablet",
            "get_gestures" => "ldp.input.gestures",
            _ => "ldp.input.pointer", // get_pointer
        };
        ctx.create_object(id, interface, request.version)?;
        // The input binding (Phase 33): the mint records the client's
        // device object for the router.
        let client = ctx.client_id();
        let keymap_fd = {
            let mut world = self.shared.world.lock().expect("world lock");
            world.input.note_device(client, request.op.name, id)
        };
        if let Some(fd) = keymap_fd {
            // `keyboard.keymap` — the descriptor rides the message
            // (argument 0 is the fd index), then `repeat_info`.
            let mut fds = FdList::new();
            fds.push(fd);
            ctx.emit_fd(id, "keymap", vec![Value::Fd(0), Value::Enum(1)], &mut fds)?;
            ctx.emit(id, "repeat_info", vec![Value::Int32(33), Value::Int32(500)])?;
        }
        Ok(())
    }

    /// The shell's window-role surface: `get_toplevel` (Phase 31's
    /// minimal service surface), `get_popup` (Phase 36 — the
    /// anchor/gravity role the constraint solver serves), and
    /// `get_dialog` (Phase 48 — the transient window role the modal
    /// machine serves: centered over its parent, gating the parent's
    /// tree while modal).
    /// `get_toplevel` mints the toplevel object and sends the
    /// *initial configure* — empty states, size 0x0 (the client's
    /// preferred size, the xdg-shape doctrine: the first buffer the
    /// client commits is the window's own answer). The serial draws
    /// from the seat's interaction clock (Phase 32): it is the one
    /// serial a client can see and later reference — the data
    /// family's `set_selection` gate.
    ///
    /// `get_popup` mints the popup object and registers it with the
    /// world's [`crate::shell::PopupHost`] — the placement proposal
    /// arrives at the popup surface's first attach (the buffer's
    /// size is the solver's input, the same doctrine as toplevel
    /// placement). `get_dialog` mints the dialog object and
    /// registers it with the [`crate::shell::DialogHost`] — the
    /// initial size proposal (0x0, the client's own answer) rides
    /// the machine's serial clock, and the centered placement
    /// arrives at the dialog surface's first attach.
    fn shell_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        match request.op.name {
            "get_toplevel" => self.shell_get_toplevel(ctx, request),
            "get_popup" => self.shell_get_popup(ctx, request),
            "get_dialog" => self.shell_get_dialog(ctx, request),
            "switch_workspace" => self.shell_switch_workspace(ctx, request),
            _ => Ok(()), // consumed: the shell has no other requests
        }
    }

    /// The `switch_workspace` arm (Phase 51 — the view switch, the
    /// taskbar's line): one client asks the seat to view another
    /// space. The world switches (clamped), every window's
    /// visibility syncs (the old space's ink leaves, the new space's
    /// shows — the set_workspace arm's own machinery), the keys land
    /// on the frontmost window of the newly-viewed space, and every
    /// shell binder learns the actual space over the wire (the
    /// asker directly here, the others through their outboxes — the
    /// next wake drains them).
    fn shell_switch_workspace(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let Value::Uint32(index) = request.args[0] else {
            return Err(bad_shape("shell.switch_workspace"));
        };
        let client = ctx.client_id();
        let actual = {
            let mut world = self.shared.world.lock().expect("world lock");
            let Some((actual, binders)) = world.switch_workspace(index) else {
                // The no-op: the seat already views the asked space
                // (the clamp may have landed it there). The request
                // is consumed silently — the truth did not change,
                // nothing sweeps, nothing is emitted.
                return Ok(());
            };
            // Every *other* binder learns through its outbox (the
            // asker's reply rides this dispatch's own emission below;
            // the sweep's focus proposals ride the outboxes too — the
            // next wake drains them in order).
            for (binder_client, object) in binders {
                if binder_client == client {
                    continue;
                }
                world.outboxes.push(OutboxEntry::event(
                    binder_client,
                    object,
                    "workspace_switched",
                    vec![Value::Uint32(actual)],
                ));
            }
            actual
        };
        ctx.emit(
            request.object,
            "workspace_switched",
            vec![Value::Uint32(actual)],
        )?;
        Ok(())
    }

    /// The `get_toplevel` arm (Phase 31's service surface; Phase 45
    /// records the role object so `toplevel.set_material` can find
    /// its surface; Phase 49 serves the states arm: the host entry,
    /// the spaces home, and the initial proposal from the machine).
    fn shell_get_toplevel(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        // (surface, decoration, id) — the schema's three.
        let Value::Object(surface_arg) = &request.args[0] else {
            return Err(bad_shape("shell.get_toplevel"));
        };
        let Value::Enum(decoration_wire) = request.args[1] else {
            return Err(bad_shape("shell.get_toplevel"));
        };
        let Value::NewId(id) = request.args[2] else {
            return Err(bad_shape("shell.get_toplevel"));
        };
        let Some(decoration) = ldp_shell::ssd::DecorationMode::from_wire(decoration_wire) else {
            return Err(out_of_range(decoration_wire));
        };
        let client = ctx.client_id();
        let proposal = {
            let mut world = self.shared.world.lock().expect("world lock");
            // Phase 45: the toplevel object maps back to its surface
            // (a null or unknown surface argument is refused by name —
            // the honest error, never a silent drop).
            let surface = if let Some(surface_obj) = *surface_arg {
                if world.scene.surface_of(client, surface_obj).is_none() {
                    return Err(LdpError::protocol(
                        ErrorCode::InvalidObject,
                        Some(surface_obj),
                        "get_toplevel: the surface argument is not a surface of this connection",
                    ));
                }
                world
                    .scene
                    .register_toplevel(client, surface_obj, id)
                    .expect("the surface was just validated")
            } else {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    None,
                    "get_toplevel: the surface argument is null",
                ));
            };
            // Phase 49 — the states arm's mint, the DialogHost
            // doctrine: the host owns the machine (the intent flags,
            // the strict two-phase proposals), records the client's
            // decoration choice (the insets' input — the wire
            // argument was previously discarded), and the spaces
            // machine takes the window's home (space 0, the seat's
            // active one).
            world
                .toplevels
                .mint(client.as_u32(), id.as_u32(), surface, decoration);
            world
                .spaces
                .assign(ldp_shell::WindowKey::new(surface.raw()), 0);
            // The initial proposal under the seat's interaction clock
            // (Phase 32's doctrine, unchanged: this is the one serial
            // a client can see and later reference — the data
            // family's `set_selection` gate). The machine adopts it
            // and continues the domain itself (`propose_at`).
            let serial = world.next_seat_serial();
            let policy = world.toplevel_policy(surface, None);
            world
                .toplevels
                .entry_mut(client.as_u32(), id.as_u32())
                .expect("the toplevel was just minted")
                .machine
                .propose_at(&policy, ldp_shell::serial::Serial(serial))
        };
        ctx.create_object(id, "ldp.shell.toplevel", request.version)?;
        // The initial configure: the client picks its own size (0x0),
        // no states, the insets the decoration mode reserves (CSD's
        // system hit zone — the client lands on the system grid; the
        // SSD chrome), workspace 0, no output pinned yet — the full
        // ten-argument proposal the schema declares (a partial
        // emission is a wire violation the client's validator
        // rejects). The machine owns it: the client's ack is the
        // strict two-phase handshake, exactly the dialog's.
        ctx.emit(id, "configure", Self::toplevel_configure_args(&proposal))?;
        Ok(())
    }

    /// The `get_popup` arm (Phase 36): mint the popup object, register
    /// the machine with the host. The placement proposal fires at the
    /// popup surface's first attach — the solver needs the buffer's
    /// size, and the position lands with the mapping commit (a menu
    /// never appears at the origin and jumps).
    fn shell_get_popup(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        // (surface, parent, anchor_rect, anchor, gravity, offset_x,
        // offset_y, constraints, id) — the schema's nine.
        let Value::Object(Some(surface_obj)) = request.args[0] else {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                match &request.args[0] {
                    Value::Object(o) => *o,
                    _ => None,
                },
                "get_popup: the surface argument is null",
            ));
        };
        let client = ctx.client_id();
        let parent = match &request.args[1] {
            Value::Object(None) => None,
            Value::Object(Some(p)) => Some(*p),
            _ => return Err(bad_shape("shell.get_popup")),
        };
        let Value::NewId(id) = request.args[8] else {
            return Err(bad_shape("shell.get_popup"));
        };
        let (anchor_rect, anchor, gravity, offset, constraints) = {
            let mut world = self.shared.world.lock().expect("world lock");
            // The role surface must be a live surface of this client.
            let Some(surface) = world.scene.surface_of(client, surface_obj) else {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(surface_obj),
                    "get_popup: the surface argument is not a surface of this connection",
                ));
            };
            let parent_surface = match parent {
                None => None,
                Some(p) => Some(world.scene.surface_of(client, p).ok_or_else(|| {
                    LdpError::protocol(
                        ErrorCode::InvalidObject,
                        Some(p),
                        "get_popup: the parent argument is not a surface of this connection",
                    )
                })?),
            };
            // The anchor geometry: rect, anchor edge, gravity, offset —
            // the value domains the solver speaks.
            let anchor_rect = match &request.args[2] {
                Value::Rect(r) => *r,
                _ => return Err(bad_shape("shell.get_popup")),
            };
            let anchor = match &request.args[3] {
                Value::Enum(a) => {
                    ldp_shell::Anchor::from_wire(*a).ok_or_else(|| out_of_range(*a))?
                }
                _ => return Err(bad_shape("shell.get_popup")),
            };
            let gravity = match &request.args[4] {
                Value::Enum(g) => {
                    ldp_shell::Gravity::from_wire(*g).ok_or_else(|| out_of_range(*g))?
                }
                _ => return Err(bad_shape("shell.get_popup")),
            };
            let (ox, oy) = match (&request.args[5], &request.args[6]) {
                (Value::Int32(x), Value::Int32(y)) => (*x, *y),
                _ => return Err(bad_shape("shell.get_popup")),
            };
            let constraints = match &request.args[7] {
                Value::Bitset(b) => ldp_shell::PopupConstraints(*b),
                _ => return Err(bad_shape("shell.get_popup")),
            };
            world.popups.mint(
                client.as_u32(),
                id.as_u32(),
                surface,
                parent_surface,
                ldp_shell::PopupGeometry {
                    anchor_rect,
                    anchor,
                    gravity,
                    offset: (ox, oy),
                },
                constraints,
            );
            (anchor_rect, anchor, gravity, (ox, oy), constraints)
        };
        let _ = (anchor_rect, anchor, gravity, offset, constraints);
        ctx.create_object(id, "ldp.shell.popup", request.version)?;
        Ok(())
    }

    /// The `get_dialog` arm (Phase 48 — the frozen surface, finally
    /// served): `(surface, parent_toplevel, modality, id)`.
    ///
    /// The parent argument must address a toplevel of this
    /// connection (the dialog's gating target and centering anchor —
    /// the *window* it belongs to). The surface may take at most one
    /// role (the spec's clause): a surface already carrying a
    /// toplevel, popup, dialog, or subsurface role is refused by
    /// name, never a silent no-op. The modality is the client's
    /// claim; the machine carries it and the input path enforces it.
    ///
    /// The initial proposal is the xdg doctrine's: size 0x0 — the
    /// client's own answer — under the machine's serial clock (the
    /// dialog's own two-phase commit, not the seat's interaction
    /// clock: the dialog's acks validate against its machine, so the
    /// serials must draw from it).
    fn shell_get_dialog(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        // (surface, parent_toplevel, modality, id) — the schema's four.
        let Value::Object(Some(surface_obj)) = request.args[0] else {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                match &request.args[0] {
                    Value::Object(o) => *o,
                    _ => None,
                },
                "get_dialog: the surface argument is null",
            ));
        };
        let Value::Object(parent_arg) = &request.args[1] else {
            return Err(bad_shape("shell.get_dialog"));
        };
        let Value::Enum(modality_wire) = request.args[2] else {
            return Err(bad_shape("shell.get_dialog"));
        };
        let Value::NewId(id) = request.args[3] else {
            return Err(bad_shape("shell.get_dialog"));
        };
        let client = ctx.client_id();
        let Some(modality) = ldp_shell::dialog::DialogModality::from_wire(modality_wire) else {
            return Err(out_of_range(modality_wire));
        };
        let proposal = {
            let mut world = self.shared.world.lock().expect("world lock");
            // The role surface must be a live surface of this client.
            let Some(surface) = world.scene.surface_of(client, surface_obj) else {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(surface_obj),
                    "get_dialog: the surface argument is not a surface of this connection",
                ));
            };
            // The parent must be a toplevel of this connection.
            let Some(parent_obj) = *parent_arg else {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    None,
                    "get_dialog: the parent_toplevel argument is null",
                ));
            };
            let Some(parent_surface) = world.scene.toplevel_of(client, parent_obj) else {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(parent_obj),
                    "get_dialog: the parent_toplevel argument is not a toplevel of this connection",
                ));
            };
            // One role per surface (the spec's clause): the toplevel's
            // role object, the popup host's membership, the dialog
            // host's own, and the tree's subsurface parentage each
            // name a live role.
            let has_role = world
                .scene
                .routes
                .get(&surface)
                .is_some_and(|r| r.toplevel_obj.is_some())
                || world.popups.is_popup_surface(surface)
                || world.dialogs.is_dialog_surface(surface)
                || world.scene.tree.parent_of(surface).is_some();
            if has_role {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidState,
                    Some(surface_obj),
                    "get_dialog: the surface already carries a role — \
                     a surface may take at most one",
                ));
            }
            // Mint the machine, then propose the initial size (0x0 —
            // the client's own answer) over the parent's content area
            // and the desktop's usable area (the centering clamps the
            // proposal the same way it clamps the placement).
            world.dialogs.mint(
                client.as_u32(),
                id.as_u32(),
                surface,
                parent_surface,
                modality,
            );
            let snapshot = world.scene.snapshot();
            let parent_content = snapshot
                .node(parent_surface)
                .map_or(Rect::new(0, 0, 0, 0), |n| n.bounds);
            let usable = world.shell.usable_physical();
            let Some(entry) = world.dialogs.entry_mut(client.as_u32(), id.as_u32()) else {
                unreachable!("the dialog was just minted");
            };
            entry.machine.propose((0, 0), parent_content, usable)
        };
        ctx.create_object(id, "ldp.shell.dialog", request.version)?;
        // The initial configure: the proposal's serial and size.
        ctx.emit(
            id,
            "configure",
            vec![
                Value::Uint32(proposal.serial.0),
                Value::Uint32(proposal.width),
                Value::Uint32(proposal.height),
            ],
        )?;
        Ok(())
    }

    /// The dialog's client-side vocabulary (Phase 48): `set_title`
    /// (the bounded-string doctrine) and `ack_configure` (the strict
    /// two-phase commit — a stale serial is the protocol error the
    /// machine's own doctrine names, unlike the popup's advisory
    /// placement: a dialog's size proposal is a contract the client
    /// realizes, so the ack must reference the live one).
    fn dialog_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id().as_u32();
        let object = request.object;
        match request.op.name {
            "set_title" => {
                let Value::String(title) = &request.args[0] else {
                    return Err(bad_shape("dialog.set_title"));
                };
                let mut world = self.shared.world.lock().expect("world lock");
                if let Some(entry) = world.dialogs.entry_mut(client, object.as_u32()) {
                    entry.machine.set_title(title).map_err(|_| {
                        // The bounded-string doctrine's own codes:
                        // over-length is the resource ceiling, an
                        // embedded NUL is the string's own invalid.
                        let code = if title.contains('\0') {
                            ErrorCode::InvalidString
                        } else {
                            ErrorCode::LimitExceeded
                        };
                        LdpError::protocol(
                            code,
                            Some(object),
                            "set_title: the title exceeds the bounded-string budget",
                        )
                    })?;
                }
                Ok(())
            }
            "ack_configure" => {
                let Value::Uint32(serial) = request.args[0] else {
                    return Err(bad_shape("dialog.ack_configure"));
                };
                let mut world = self.shared.world.lock().expect("world lock");
                if let Some(entry) = world.dialogs.entry_mut(client, object.as_u32()) {
                    entry
                        .machine
                        .ack_configure(ldp_shell::serial::Serial(serial))
                        .map_err(|_| {
                            LdpError::protocol(
                                ErrorCode::InvalidState,
                                Some(object),
                                "ack_configure: the serial is not the live proposal",
                            )
                        })?;
                }
                Ok(())
            }
            _ => Ok(()), // consumed: no other dialog requests
        }
    }

    /// The popup's client-side vocabulary (Phase 36): `grab` (the
    /// serial-freshness doctrine — the serial must equal the seat's
    /// interaction clock, the same rule the data family enforces),
    /// `ack_configure` (consumed — placement is advisory), and
    /// `reposition` / `dismiss` (the driver arms below).
    fn popup_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id().as_u32();
        let object = request.object;
        match request.op.name {
            "grab" => {
                let (Value::Object(seat), Value::Uint32(serial)) =
                    (&request.args[0], &request.args[1])
                else {
                    return Err(bad_shape("popup.grab"));
                };
                let Some(seat_obj) = *seat else {
                    return Err(LdpError::protocol(
                        ErrorCode::InvalidObject,
                        None,
                        "grab: the seat argument is null",
                    ));
                };
                match ctx.store().lookup(seat_obj) {
                    ldp_server::object::Lookup::Live(entry)
                        if entry.interface == "ldp.input.seat" => {}
                    _ => {
                        return Err(LdpError::protocol(
                            ErrorCode::InvalidInterface,
                            Some(seat_obj),
                            "grab: the object is not a seat binding",
                        ));
                    }
                }
                let mut world = self.shared.world.lock().expect("world lock");
                let press = world.seat_serial;
                let entry = world
                    .popups
                    .entry_mut(client, object.as_u32())
                    .ok_or_else(|| unknown_object(object))?;
                // The freshness doctrine: the grab serial must match
                // the seat's current interaction serial (the same
                // equality `set_selection` enforces; the button-press
                // serial record arms with the device-event feed).
                entry
                    .machine
                    .grab(
                        Some(ldp_shell::serial::Serial(press)),
                        ldp_shell::serial::Serial(*serial),
                    )
                    .map_err(|e| {
                        LdpError::protocol(
                            ErrorCode::InvalidState,
                            Some(object),
                            format!("grab: {e}"),
                        )
                    })?;
                Ok(())
            }
            "ack_configure" => {
                let Value::Uint32(serial) = request.args[0] else {
                    return Err(bad_shape("popup.ack_configure"));
                };
                let mut world = self.shared.world.lock().expect("world lock");
                if let Some(entry) = world.popups.entry_mut(client, object.as_u32()) {
                    // Any serial the client saw is acceptable
                    // (placement is advisory); the machine records it.
                    let _ = entry
                        .machine
                        .ack_configure(ldp_shell::serial::Serial(serial));
                }
                Ok(())
            }
            "reposition" => self.popup_reposition(ctx, request),
            "dismiss" => {
                let mut world = self.shared.world.lock().expect("world lock");
                // The client-initiated dismissal (a menu item
                // activated): the machine records it; a double
                // dismissal is a no-op, never an error.
                let _ = world
                    .popups
                    .entry_mut(client, object.as_u32())
                    .is_some_and(|e| {
                        let live = e.machine.is_live();
                        e.machine.dismiss();
                        live
                    });
                drop(world);
                // `popup.done` — the dismissal confirmation (the spec:
                // the client stops rendering it).
                ctx.emit(object, "done", vec![])?;
                Ok(())
            }
            _ => Ok(()), // consumed: no other popup requests
        }
    }

    /// The `reposition` arm: new anchor geometry, re-solve against the
    /// current world (the parent's origin and the shell's usable
    /// area), emit the fresh proposal, and apply the position now (a
    /// reposition mid-life moves the mapped popup immediately — the
    /// re-layout arm's `set_position_now` doctrine).
    fn popup_reposition(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id().as_u32();
        let object = request.object;
        let anchor_rect = match &request.args[0] {
            Value::Rect(r) => *r,
            _ => return Err(bad_shape("popup.reposition")),
        };
        let anchor = match &request.args[1] {
            Value::Enum(a) => ldp_shell::Anchor::from_wire(*a).ok_or_else(|| out_of_range(*a))?,
            _ => return Err(bad_shape("popup.reposition")),
        };
        let gravity = match &request.args[2] {
            Value::Enum(g) => ldp_shell::Gravity::from_wire(*g).ok_or_else(|| out_of_range(*g))?,
            _ => return Err(bad_shape("popup.reposition")),
        };
        let (ox, oy) = match (&request.args[3], &request.args[4]) {
            (Value::Int32(x), Value::Int32(y)) => (*x, *y),
            _ => return Err(bad_shape("popup.reposition")),
        };
        let mut world = self.shared.world.lock().expect("world lock");
        let Some(entry) = world.popups.entry_mut(client, object.as_u32()) else {
            return Err(unknown_object(object));
        };
        entry
            .machine
            .reposition(ldp_shell::PopupGeometry {
                anchor_rect,
                anchor,
                gravity,
                offset: (ox, oy),
            })
            .map_err(|e| {
                LdpError::protocol(
                    ErrorCode::InvalidState,
                    Some(object),
                    format!("reposition: {e}"),
                )
            })?;
        // Re-solve with the standing size against the standing world.
        let size = entry.solved_size();
        let surface = entry.surface;
        let parent = entry.parent;
        let usable = popup_usable(&world, parent);
        let proposal = world
            .popups
            .repropose_force(client, object.as_u32(), size, usable);
        // Apply the position now (a mapped popup moves immediately).
        if let Some((_, placement)) = &proposal {
            let origin = parent.map(|p| root_origin(&world, p));
            let (px, py) = origin.unwrap_or((0, 0));
            world
                .scene
                .tree
                .set_position_now(surface, px + placement.x, py + placement.y)
                .ok();
        }
        drop(world);
        if let Some((serial, placement)) = proposal {
            ctx.emit(
                object,
                "configure",
                vec![
                    Value::Uint32(serial),
                    Value::Int32(placement.x),
                    Value::Int32(placement.y),
                    Value::Uint32(placement.width),
                    Value::Uint32(placement.height),
                ],
            )?;
        }
        Ok(())
    }

    /// The toplevel's client-side vocabulary — the states arm served
    /// (Phase 49): every request the frozen schema declares is live.
    ///
    /// * the hints — `set_title`, `set_app_id` (the bounded-string
    ///   doctrine's own error codes), `set_min_size`, `set_max_size`
    ///   — land on the machine (the window's identity and size
    ///   grammar, kept consistent by construction, the taskbar's
    ///   future material ready);
    /// * the geometry verbs — `maximize`/`unmaximize`,
    ///   `fullscreen(output|null)`/`unfullscreen` — propose through
    ///   the two-phase commit: the `configure` carries the derived
    ///   content size and the insets the decoration mode reserves,
    ///   the client `ack_configure`s, and its next commit realizes
    ///   the placement (the buffer the client attaches is the size
    ///   answer; the position completes it — a window lands placed,
    ///   never origin-then-jump);
    /// * the visibility verbs — `minimize`/`unminimize`,
    ///   `set_workspace`, `set_sticky` — apply immediately (the
    ///   shell's own visual decision: a hidden window renders
    ///   nowhere, takes no input, parks its frame requests — App
    ///   Nap's own seam — and leaves its outputs over the wire;
    ///   `workspace_changed` reports the actual space; sticky
    ///   windows show on every one);
    /// * `ack_configure` — strict: a serial that is not the live
    ///   proposal is the protocol error the two-phase contract names
    ///   (the dialog's own doctrine, now the toplevel's too);
    /// * Phase 45's `set_material` and Phase 47's semantic triple
    ///   ride alongside (what the surface *is*, what it may *expose*,
    ///   how the machine spends its frame budget on it).
    ///
    /// The toplevel object must map back to a live host entry of
    /// this connection (the mint happened at `get_toplevel`); every
    /// enum is validated against the schema's domain, and the server
    /// keeps its own invariants over the claims.
    fn toplevel_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        match request.op.name {
            "set_material" | "set_semantic_role" | "set_security_class" | "set_scene_profile" => {
                self.toplevel_semantic_request(ctx, request)
            }
            "set_title" | "set_app_id" | "set_min_size" | "set_max_size" | "ack_configure" => {
                self.toplevel_hint_request(ctx, request)
            }
            "maximize" | "unmaximize" | "fullscreen" | "unfullscreen" | "minimize"
            | "unminimize" | "set_sticky" | "set_workspace" => {
                self.toplevel_state_request(ctx, request)
            }
            "start_move" | "start_resize" => self.toplevel_drag_request(ctx, request),
            _ => Ok(()), // consumed: the schema declares no other requests
        }
    }

    /// The semantic quartet (Phases 45/47): `set_material`,
    /// `set_semantic_role`, `set_security_class`, `set_scene_profile`
    /// — one enum argument each, resolved to the surface through the
    /// scene's role-object mapping (the register happened at
    /// `get_toplevel`).
    fn toplevel_semantic_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        // Every live member carries exactly one enum argument — the
        // schema's shape for all four.
        let Value::Enum(wire) = request.args[0] else {
            return Err(bad_shape("toplevel semantic request"));
        };
        let client = ctx.client_id();
        let mut world = self.shared.world.lock().expect("world lock");
        let surface = world
            .scene
            .toplevel_of(client, request.object)
            .ok_or_else(|| {
                LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(request.object),
                    "the toplevel object is not a toplevel of this connection",
                )
            })?;
        match request.op.name {
            "set_material" => {
                // The wire enum → the material family: 1 is the clear
                // (the server's own resolution rules dress the window
                // again); 2..=6 map through the typed vocabulary's
                // `from_wire`, the value the schema's domain carries.
                let material = if wire == 1 {
                    // The clear: no claim, the server's own resolution.
                    None
                } else {
                    use ldp_shell::toplevel::Material as WireMaterial;
                    let Some(m) = WireMaterial::from_wire(wire) else {
                        return Err(out_of_range(wire));
                    };
                    Some(match m {
                        WireMaterial::Panel => ldp_renderer::Material::Panel,
                        WireMaterial::Sheet => ldp_renderer::Material::Sheet,
                        WireMaterial::Menu => ldp_renderer::Material::Menu,
                        WireMaterial::VibrantDark => ldp_renderer::Material::VibrantDark,
                        WireMaterial::Chrome => ldp_renderer::Material::Chrome,
                    })
                };
                world.scene.set_material(surface, material);
            }
            "set_semantic_role" => {
                use ldp_compositor::semantics::SemanticRole;
                use ldp_shell::toplevel::SemanticRole as WireRole;
                // Wire 1 (`window`) is the plain doctrine — the clear
                // (no claim); 2..=5 map through the wire vocabulary.
                let role = if wire == 1 {
                    None
                } else {
                    let Some(r) = WireRole::from_wire(wire) else {
                        return Err(out_of_range(wire));
                    };
                    Some(match r {
                        WireRole::Dialog => SemanticRole::Dialog,
                        WireRole::Tooltip => SemanticRole::Tooltip,
                        WireRole::Overlay => SemanticRole::Overlay,
                        WireRole::Lock => SemanticRole::Lock,
                        WireRole::Window => SemanticRole::Window,
                    })
                };
                world.scene.set_semantic_role(surface, role);
            }
            "set_security_class" => {
                use ldp_compositor::semantics::SecurityClass;
                use ldp_shell::toplevel::SecurityClass as WireClass;
                // Wire 1 (`normal`) is the plain doctrine — the clear;
                // 2..=4 map through the wire vocabulary.
                let class = if wire == 1 {
                    None
                } else {
                    let Some(c) = WireClass::from_wire(wire) else {
                        return Err(out_of_range(wire));
                    };
                    Some(match c {
                        WireClass::Private => SecurityClass::Private,
                        WireClass::Protected => SecurityClass::Protected,
                        WireClass::System => SecurityClass::System,
                        WireClass::Normal => SecurityClass::Normal,
                    })
                };
                world.scene.set_security_class(surface, class);
            }
            "set_scene_profile" => {
                use ldp_compositor::semantics::SceneProfile;
                use ldp_shell::toplevel::SceneProfile as WireProfile;
                // Wire 1 (`desktop`) IS a claim (the explicit identity
                // — the operator's policy stands); 2..=3 the others.
                let Some(p) = WireProfile::from_wire(wire) else {
                    return Err(out_of_range(wire));
                };
                let profile = match p {
                    WireProfile::Desktop => SceneProfile::Desktop,
                    WireProfile::Creative => SceneProfile::Creative,
                    WireProfile::Gaming => SceneProfile::Gaming,
                };
                let now = world.now();
                world.scene.set_scene_profile(surface, profile, now);
            }
            _ => unreachable!("the router above filtered the op name"),
        }
        drop(world);
        let _ = ctx;
        Ok(())
    }

    /// The hints and the handshake (Phase 49): the bounded strings
    /// (`set_title`, `set_app_id` — the dialog `set_title`'s own
    /// error split: an embedded NUL is the string's own invalid,
    /// over-length is the resource ceiling), the size pair
    /// (`set_min_size`/`set_max_size`, clamped against each other by
    /// the machine), and `ack_configure` (strict: the serial must be
    /// the live proposal). A dead object's hints are consumed
    /// silently — the destroy already told the client (the dialog
    /// host's own doctrine).
    fn toplevel_hint_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id().as_u32();
        let object = request.object;
        let mut world = self.shared.world.lock().expect("world lock");
        let Some(entry) = world.toplevels.entry_mut(client, object.as_u32()) else {
            return Ok(()); // consumed: the object is dead (the dialog doctrine)
        };
        match request.op.name {
            "set_title" | "set_app_id" => {
                let Value::String(text) = &request.args[0] else {
                    return Err(bad_shape("toplevel string request"));
                };
                let result = if request.op.name == "set_title" {
                    entry.machine.set_title(text)
                } else {
                    entry.machine.set_app_id(text)
                };
                result.map_err(|_| {
                    // The bounded-string doctrine's own codes (the
                    // dialog arm's split, verbatim).
                    let code = if text.contains('\0') {
                        ErrorCode::InvalidString
                    } else {
                        ErrorCode::LimitExceeded
                    };
                    LdpError::protocol(
                        code,
                        Some(object),
                        "the string argument exceeds the bounded-string budget",
                    )
                })?;
            }
            "set_min_size" | "set_max_size" => {
                let (Value::Uint32(w), Value::Uint32(h)) = (&request.args[0], &request.args[1])
                else {
                    return Err(bad_shape("toplevel size request"));
                };
                if request.op.name == "set_min_size" {
                    entry.machine.set_min_size(*w, *h);
                } else {
                    entry.machine.set_max_size(*w, *h);
                }
            }
            "ack_configure" => {
                let Value::Uint32(serial) = request.args[0] else {
                    return Err(bad_shape("toplevel.ack_configure"));
                };
                entry
                    .machine
                    .ack_configure(ldp_shell::serial::Serial(serial))
                    .map_err(|_| {
                        LdpError::protocol(
                            ErrorCode::InvalidState,
                            Some(object),
                            "ack_configure: the serial is not the live proposal",
                        )
                    })?;
            }
            _ => unreachable!("the router above filtered the op name"),
        }
        drop(world);
        let _ = ctx;
        Ok(())
    }

    /// The states arm's verbs (Phase 49): the intents, the spaces
    /// truth, and the proposal — one shared driver under one lock
    /// ([`Self::drive_toplevel_state`]), then the emissions (the
    /// `configure` proposal, plus `workspace_changed` when the move
    /// made one).
    fn toplevel_state_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id().as_u32();
        let object = request.object;
        // The verb's argument, decoded by the schema's shape.
        let intent = match request.op.name {
            "maximize" => ToplevelIntent::Maximize,
            "unmaximize" => ToplevelIntent::Unmaximize,
            "fullscreen" => {
                // (output: object, nullable) — null: the current
                // (primary) output.
                let Value::Object(output) = &request.args[0] else {
                    return Err(bad_shape("toplevel.fullscreen"));
                };
                ToplevelIntent::Fullscreen(*output)
            }
            "unfullscreen" => ToplevelIntent::Unfullscreen,
            "minimize" => ToplevelIntent::Minimize,
            "unminimize" => ToplevelIntent::Unminimize,
            "set_sticky" => {
                let Value::Bool(sticky) = request.args[0] else {
                    return Err(bad_shape("toplevel.set_sticky"));
                };
                ToplevelIntent::Sticky(sticky)
            }
            "set_workspace" => {
                let Value::Uint32(index) = request.args[0] else {
                    return Err(bad_shape("toplevel.set_workspace"));
                };
                ToplevelIntent::Workspace(index)
            }
            _ => unreachable!("the router above filtered the op name"),
        };
        let driven = {
            let mut world = self.shared.world.lock().expect("world lock");
            Self::drive_toplevel_state(&mut world, client, object, intent)?
        };
        let Some((proposal, workspace_changed)) = driven else {
            return Ok(()); // consumed: the object is dead (nothing to emit)
        };
        // The emissions: the proposal first (the machine's answer),
        // the move's report alongside it (the spec's
        // `workspace_changed`). A dead object emits nothing — the
        // destroy already told the client.
        ctx.emit(
            object,
            "configure",
            Self::toplevel_configure_args(&proposal),
        )?;
        if let Some(actual) = workspace_changed {
            ctx.emit(object, "workspace_changed", vec![Value::Uint32(actual)])?;
        }
        Ok(())
    }

    /// The states arm's shared driver: land the intent on the
    /// machine, land the spaces truth (the move, the stickiness),
    /// resolve the fullscreen pin, propose under the live policy,
    /// and sync the visibility truth — one lock, one narrative.
    /// Returns the proposal to emit and the `workspace_changed`
    /// payload (`None` when the verb moved nothing); `None` itself
    /// when the object is dead (the dialog host's doctrine — the
    /// destroy already told the client, nothing is emitted).
    ///
    /// # Errors
    /// [`ErrorCode::InvalidObject`] when a pinned `fullscreen`
    /// output is not a bound output of the connection.
    fn drive_toplevel_state(
        world: &mut World,
        client: u32,
        object: ObjectId,
        intent: ToplevelIntent,
    ) -> Result<Option<(ldp_shell::toplevel::Configure, Option<u32>)>> {
        let Some(surface) = world
            .toplevels
            .entry(client, object.as_u32())
            .map(|t| t.surface)
        else {
            return Ok(None); // consumed: the object is dead
        };
        let key = ldp_shell::WindowKey::new(surface.raw());
        // Phase 50 — the operator's hand meets the states arm: the
        // verb's regime is authoritative on *this* window. The
        // engaging verbs (maximize, fullscreen, minimize, a workspace
        // move) end a live drag on the dragged window outright — the
        // states take the geometry back; the hints, stickiness, and
        // the un-verbs ride along, never killing a grip.
        let engaging = matches!(
            intent,
            ToplevelIntent::Maximize
                | ToplevelIntent::Fullscreen(_)
                | ToplevelIntent::Minimize
                | ToplevelIntent::Workspace(_)
        );
        if engaging && world.drags.surface() == Some(surface) {
            world.end_drag_silent();
        }
        // The spaces truth first (the policy reads the fresh
        // assignment): the move, the stickiness.
        let workspace_changed = match intent {
            ToplevelIntent::Workspace(index) => Some(world.spaces.move_window(key, index)),
            ToplevelIntent::Sticky(sticky) => {
                world.spaces.set_sticky(key, sticky);
                None
            }
            _ => None,
        };
        // The fullscreen pin: the argument names an output the
        // client bound (refused by name otherwise); null resolves to
        // the primary — the client's own bound object for it, when
        // it holds one (an unbound client gets an unpinned proposal;
        // the realize falls back to the primary).
        let pin = match intent {
            ToplevelIntent::Fullscreen(Some(obj)) => {
                let bound = world
                    .scene
                    .output_binds
                    .iter()
                    .any(|(_, bound)| *bound == obj);
                if !bound {
                    return Err(LdpError::protocol(
                        ErrorCode::InvalidObject,
                        Some(obj),
                        "fullscreen: the output argument is not a bound output of this connection",
                    ));
                }
                Some(obj)
            }
            ToplevelIntent::Fullscreen(None) => world
                .outputs
                .first()
                .and_then(|slot| world.scene.output_binds.get(&(client, slot.crtc)).copied()),
            _ => None,
        };
        // The intent lands on the machine, then the proposal (one
        // borrow, one narrative).
        let policy = world.toplevel_policy(surface, pin);
        // Phase 50: the floating geometry at the moment of the
        // engagement (the tree's realized truth — the buffer the
        // client actually committed; the machine's applied configure
        // carries the handshake's 0x0 "client chooses" for the first
        // float, so the tree is the honest source).
        let floating_rect = world.scene.tree.get(surface).map(Surface::last_bounds);
        let proposal = {
            let Some(entry) = world.toplevels.entry_mut(client, object.as_u32()) else {
                unreachable!("the entry was just found");
            };
            // Phase 50: the floating size at the moment of the geometry
            // engagement — captured once (beside the restore point the
            // realize takes), the demotion's restore answer; and the
            // verb's own regime voids any drag-held position (the
            // proposal that follows is the verb's, never the hand's).
            if matches!(
                intent,
                ToplevelIntent::Maximize | ToplevelIntent::Fullscreen(_)
            ) && !entry.machine.wanted().maximized()
                && !entry.machine.wanted().fullscreen()
                && entry.restore_size.is_none()
            {
                entry.restore_size = floating_rect.filter(|r| r.w > 0 && r.h > 0).map(|r| {
                    let lw = crate::shell::unscale_axis(r.w as i32, policy.scale).max(1);
                    let lh = crate::shell::unscale_axis(r.h as i32, policy.scale).max(1);
                    (
                        u32::try_from(lw).unwrap_or(1),
                        u32::try_from(lh).unwrap_or(1),
                    )
                });
            }
            entry.drag_pos = None;
            match intent {
                ToplevelIntent::Maximize => entry.machine.maximize(),
                ToplevelIntent::Unmaximize => entry.machine.unmaximize(),
                ToplevelIntent::Fullscreen(_) => entry.machine.fullscreen(),
                ToplevelIntent::Unfullscreen => entry.machine.unfullscreen(),
                ToplevelIntent::Minimize => entry.machine.minimize(),
                ToplevelIntent::Unminimize => entry.machine.unminimize(),
                ToplevelIntent::Sticky(sticky) => entry.machine.set_sticky(sticky),
                ToplevelIntent::Workspace(_) => {}
            }
            entry.machine.propose(&policy)
        };
        // The visibility truth (minimize, workspace moves,
        // stickiness): the render path, the input path, the frame
        // economy, and the enter/leave wire all follow from this one
        // sync.
        world.sync_states_visibility(surface);
        Ok(Some((proposal, workspace_changed)))
    }

    /// The operator's hand (Phase 50): `toplevel.start_move` and
    /// `toplevel.start_resize` — the client asks the *compositor* to
    /// drive the geometry (the title-bar drag and the edge grip every
    /// desktop serves). The doctrine, piece by piece:
    ///
    /// * the **seat argument** must be a live seat binding of this
    ///   connection (the popup `grab` arm's own validation);
    /// * the **serial** must equal the seat's current interaction
    ///   serial — the freshness gate `set_selection` and `popup.grab`
    ///   enforce (press-issued serials ride the device-event serial
    ///   line in the roadmap); a stale serial is `invalid_state`;
    /// * the **window** must be mapped and visible (an unmapped or
    ///   hidden window has no grip — `invalid_state`);
    /// * `start_resize` on a maximized or fullscreen window is
    ///   refused (`invalid_state`): the geometry states own the size
    ///   — demote with `start_move` first (the DWM doctrine);
    /// * `start_move` on a geometry-stated window **demotes** it: the
    ///   machine releases the states, the proposal restores the
    ///   floating size the verbs displaced (`restore_size`, the
    ///   workspace's two-thirds as the never-floated fallback — the
    ///   DWM default-size doctrine), the window detaches *now* under
    ///   the pointer's proportional grip (the anchored position —
    ///   the size realizes at the client's ack+commit cadence, never
    ///   tearing), and the drag carries on from there;
    /// * **one hand**: a live drag on any window is superseded (the
    ///   retired grip ends silently, the new regime's proposals its
    ///   own);
    /// * the drag itself is the pump's to advance (the motion batches
    ///   move the window / mint the resize proposals; the button's
    ///   release retires the grip with the final proposal).
    #[allow(clippy::too_many_lines)] // one narrative: the gates, the demotion, the grip
    fn toplevel_drag_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id().as_u32();
        let object = request.object;
        let is_resize = request.op.name == "start_resize";
        // The arguments: (seat, serial) for the move, (seat, serial,
        // edges) for the resize — the schema's shapes.
        let (seat, serial, edges) = if is_resize {
            let (Value::Object(seat), Value::Uint32(serial), Value::Enum(wire)) =
                (&request.args[0], &request.args[1], &request.args[2])
            else {
                return Err(bad_shape("toplevel.start_resize"));
            };
            let edges = ldp_shell::toplevel::ResizeEdge::from_wire(*wire)
                .ok_or_else(|| out_of_range(*wire))?;
            (*seat, *serial, Some(edges))
        } else {
            let (Value::Object(seat), Value::Uint32(serial)) = (&request.args[0], &request.args[1])
            else {
                return Err(bad_shape("toplevel.start_move"));
            };
            (*seat, *serial, None)
        };
        let Some(seat_obj) = seat else {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                None,
                format!("{}: the seat argument is null", request.op.name),
            ));
        };
        match ctx.store().lookup(seat_obj) {
            ldp_server::object::Lookup::Live(entry) if entry.interface == "ldp.input.seat" => {}
            _ => {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidInterface,
                    Some(seat_obj),
                    format!("{}: the object is not a seat binding", request.op.name),
                ));
            }
        }
        let mut world = self.shared.world.lock().expect("world lock");
        // The freshness gate: the serial the client referenced must be
        // the seat's current interaction serial (the same equality
        // `set_selection` and `popup.grab` enforce — the value the
        // client learned from the seat's latest serial-bearing
        // delivery).
        if serial != world.seat_serial {
            return Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(object),
                format!(
                    "{}: the serial is not the seat's current interaction serial",
                    request.op.name
                ),
            ));
        }
        let Some(entry) = world.toplevels.entry(client, object.as_u32()) else {
            return Ok(()); // consumed: the object is dead (the dialog doctrine)
        };
        let surface = entry.surface;
        let wanted = entry.machine.wanted();
        // The window must be mapped and visible: an invisible window
        // has no grip.
        let mapped = world
            .scene
            .tree
            .get(surface)
            .is_some_and(|s| s.state().is_mapped());
        let hidden = world.scene.hidden.contains(&surface);
        if !mapped || hidden {
            return Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(object),
                format!(
                    "{}: {}",
                    request.op.name,
                    if hidden {
                        "a hidden window has no grip"
                    } else {
                        "an unmapped window has no grip"
                    }
                ),
            ));
        }
        // The resize's geometry refusal: the states own the size.
        if is_resize && (wanted.maximized() || wanted.fullscreen()) {
            return Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(object),
                "start_resize: the geometry states own the size — demote with start_move first",
            ));
        }
        // The demotion: a geometry-stated window dragged by its title
        // releases the states and restores its floating size.
        let demotion = if !is_resize && (wanted.maximized() || wanted.fullscreen()) {
            let pointer = world.input.pointer_position();
            let rect = world
                .scene
                .tree
                .get(surface)
                .map_or(Rect::new(0, 0, 1, 1), Surface::last_bounds);
            let policy = world.toplevel_policy(surface, None);
            // The restore size: the floating size at the geometry
            // engagement, the workspace's two-thirds as the
            // never-floated fallback (the DWM default-size doctrine).
            let (mut rw, mut rh) = world
                .toplevels
                .entry(client, object.as_u32())
                .and_then(|e| e.restore_size)
                .filter(|(w, h)| *w > 0 && *h > 0)
                .unwrap_or((
                    policy.workspace_area.w / 3 * 2,
                    policy.workspace_area.h / 3 * 2,
                ));
            // clamp against the hints (logical)
            let entry = world.toplevels.entry_mut(client, object.as_u32());
            if let Some(e) = entry {
                let clamped = e.machine.clamp_size(rw, rh);
                if clamped.0 > 0 && clamped.1 > 0 {
                    (rw, rh) = clamped;
                }
            }
            let phys_w = crate::shell::scale_axis(rw as i32, policy.scale);
            let phys_h = crate::shell::scale_axis(rh as i32, policy.scale);
            // The anchored start: the pointer's proportional grip on
            // the geometry-stated window maps onto the restore size
            // (the window pops under the hand; the size realizes at
            // the client's cadence — never tearing).
            let (rw_f, rh_f) = (rect.w as f32, rect.h as f32);
            let ratio_x = if rw_f > 0.0 {
                ((pointer.0 - rect.x as f32) / rw_f).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let ratio_y = if rh_f > 0.0 {
                ((pointer.1 - rect.y as f32) / rh_f).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let anchor_x = (pointer.0 - ratio_x * phys_w as f32).round() as i32;
            let anchor_y = (pointer.1 - ratio_y * phys_h as f32).round() as i32;
            // The machine releases the states; the restore point and
            // its size consumed (a fresh engagement re-captures).
            if let Some(e) = world.toplevels.entry_mut(client, object.as_u32()) {
                e.machine.unmaximize();
                e.machine.unfullscreen();
                e.machine.set_resizing(false);
                e.restore = None;
                e.restore_size = None;
                e.drag_pos = None;
            }
            // The detach: the window jumps to the anchored position
            // *now* (server truth — the geometry-stated fill leaves
            // with it).
            world
                .scene
                .tree
                .set_position_now(surface, anchor_x, anchor_y)
                .ok();
            world.scene.dirty = true;
            world.sync_states_visibility(surface);
            Some((anchor_x, anchor_y, phys_w as u32, phys_h as u32, (rw, rh)))
        } else {
            None
        };
        // The grip itself: the geometry truths the pump derives from.
        let pointer = world.input.pointer_position();
        let (window_start, last_size) = if let Some((ax, ay, pw, ph, (rw, rh))) = demotion {
            // The demoted drag anchors on the restore size's rect (the
            // buffer is still the old, big one — the client commits
            // the restore size at its cadence; the position follows
            // the anchor either way).
            (Rect::new(ax, ay, pw, ph), (rw, rh))
        } else {
            let rect = world
                .scene
                .tree
                .get(surface)
                .map_or(Rect::new(0, 0, 1, 1), Surface::last_bounds);
            let policy = world.toplevel_policy(surface, None);
            let lw = crate::shell::unscale_axis(rect.w as i32, policy.scale).max(1);
            let lh = crate::shell::unscale_axis(rect.h as i32, policy.scale).max(1);
            (
                rect,
                (
                    u32::try_from(lw).unwrap_or(1),
                    u32::try_from(lh).unwrap_or(1),
                ),
            )
        };
        let mode = if is_resize {
            crate::shell::DragMode::Resize(edges.expect("the resize arm decoded it"))
        } else {
            crate::shell::DragMode::Move
        };
        if is_resize {
            if let Some(e) = world.toplevels.entry_mut(client, object.as_u32()) {
                e.machine.set_resizing(true);
            }
        }
        let drag = crate::shell::LiveDrag {
            client,
            object: object.as_u32(),
            surface,
            mode,
            pointer_start: pointer,
            window_start,
            last_size,
            proposed: false,
        };
        world.begin_drag(drag);
        // The demotion's proposal: the floating size the verbs
        // displaced, offered through the same two-phase commit (the
        // client acks, its commit realizes the size — the drag moves
        // the position meanwhile, the anchor holds).
        let demotion_proposal = demotion.and_then(|(_, _, _, _, (rw, rh))| {
            let policy = world.toplevel_policy(surface, None);
            world
                .toplevels
                .entry_mut(client, object.as_u32())
                .map(|e| e.machine.propose_sized(&policy, (rw, rh)))
        });
        drop(world);
        if let Some(proposal) = demotion_proposal {
            ctx.emit(
                object,
                "configure",
                Self::toplevel_configure_args(&proposal),
            )?;
        }
        Ok(())
    }

    /// The ten-argument `configure` emission shape (the schema's
    /// declaration, the machine's proposal — a partial emission is a
    /// wire violation the client's validator rejects).
    fn toplevel_configure_args(c: &ldp_shell::toplevel::Configure) -> Vec<Value> {
        crate::shell::toplevel_configure_values(c)
    }

    /// The data family's manager surface (Phase 32):
    /// `get_data_device(seat, new_id)` mints the per-seat device the
    /// selection events target, `create_data_source(new_id)` mints a
    /// source under construction. The seat argument must be a live
    /// seat binding of this connection — the device belongs to that
    /// seat's coherent focus set.
    fn data_manager_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        match request.op.name {
            "get_data_device" => {
                let (Value::Object(seat), Value::NewId(id)) = (&request.args[0], &request.args[1])
                else {
                    return Err(bad_shape("data_device_manager.get_data_device"));
                };
                let Some(seat_obj) = *seat else {
                    return Err(LdpError::protocol(
                        ErrorCode::InvalidObject,
                        None,
                        "get_data_device: the seat argument is null",
                    ));
                };
                match ctx.store().lookup(seat_obj) {
                    ldp_server::object::Lookup::Live(entry)
                        if entry.interface == "ldp.input.seat" => {}
                    _ => {
                        return Err(LdpError::protocol(
                            ErrorCode::InvalidInterface,
                            Some(seat_obj),
                            "get_data_device: the object is not a seat binding",
                        ));
                    }
                }
                let id = *id;
                ctx.create_object(id, "ldp.data.data_device", request.version)?;
                let mut world = self.shared.world.lock().expect("world lock");
                world
                    .clipboard
                    .create_device(data_key(ctx.client_id()), crate::scene::SEAT, id);
                drop(world);
                Ok(())
            }
            "create_data_source" => {
                let Value::NewId(id) = request.args[0] else {
                    return Err(bad_shape("data_device_manager.create_data_source"));
                };
                ctx.create_object(id, "ldp.data.data_source", request.version)?;
                let mut world = self.shared.world.lock().expect("world lock");
                // The key is implicit in the object (the reverse
                // lookups resolve it on every later request).
                let _ = world.clipboard.create_source(data_key(ctx.client_id()), id);
                drop(world);
                Ok(())
            }
            _ => Ok(()), // consumed: the manager has no other requests
        }
    }

    /// `data_source.offer(mime)` — one MIME type joins the offer list
    /// (before any selection). The manager's own machine rejects a
    /// late offer, a flood, or a malformed MIME with its typed error;
    /// the dispatcher renders the taxonomy arm.
    fn data_source_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        if request.op.name != "offer" {
            return Ok(()); // consumed: no other source requests
        }
        let Value::String(mime) = &request.args[0] else {
            return Err(bad_shape("data_source.offer"));
        };
        let client = ctx.client_id();
        let mut world = self.shared.world.lock().expect("world lock");
        let Some(key) = world
            .clipboard
            .source_of_object(data_key(client), request.object)
        else {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(request.object),
                "data_source.offer: the object is not a data source of this client",
            ));
        };
        world
            .clipboard
            .source_offer(key, mime)
            .map_err(|e| source_error(request.object, e))
    }

    /// The data device's selection vocabulary (Phase 32):
    /// `set_selection`/`set_primary_selection` install a source on
    /// the seat's slot — the serial must equal the seat's current
    /// interaction serial (the freshness doctrine; the client learned
    /// it from the shell's configure). `start_drag` is refused with
    /// `invalid_state`: the seat's device-event feed (the evdev path)
    /// is the roadmap line that arms drag-and-drop — an honest,
    /// typed refusal instead of a drag that can never move.
    fn data_device_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id();
        let mut world = self.shared.world.lock().expect("world lock");
        let Some(_seat) = world
            .clipboard
            .seat_of_device(data_key(client), request.object)
        else {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(request.object),
                "the object is not a data device of this client",
            ));
        };
        match request.op.name {
            "set_selection" | "set_primary_selection" => {
                let (Value::Object(source), Value::Uint32(serial)) =
                    (&request.args[0], &request.args[1])
                else {
                    return Err(bad_shape("data_device.set_selection"));
                };
                let source = *source;
                let serial = *serial;
                let slot = if request.op.name == "set_selection" {
                    Slot::Clipboard
                } else {
                    Slot::Primary
                };
                let source_key = source.map(|obj| {
                    world
                        .clipboard
                        .source_of_object(data_key(client), obj)
                        .ok_or_else(|| {
                            LdpError::protocol(
                                ErrorCode::InvalidObject,
                                Some(obj),
                                "set_selection: the object is not a data source of this client",
                            )
                        })
                });
                let source_key = source_key.transpose()?;
                // The freshness gate: the serial the client referenced
                // must be the seat's current one (the manager enforces
                // the equality; the world supplies the truth).
                let input_serial = world.seat_serial;
                let mut mint = world.next_data_object;
                let batch = {
                    let mut supplier = |_client: ldp_clipboard::ClientKey| {
                        mint = mint.wrapping_add(1);
                        ObjectId::from_wire(ObjectId::SERVER_FLAG | mint)
                    };
                    world.clipboard.set_slot(
                        crate::scene::SEAT,
                        slot,
                        data_key(client),
                        source_key,
                        Some(input_serial),
                        serial,
                        &mut supplier,
                    )
                };
                world.next_data_object = mint;
                let batch = batch.map_err(|e| device_error(request.object, e))?;
                Self::route_data(ctx, client, &mut world, batch)
            }
            "start_drag" => Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(request.object),
                "start_drag: the seat's device-event feed is not served yet — \
                 drag-and-drop arms with the pointer path (the evdev roadmap line)",
            )),
            _ => Ok(()), // consumed: the device has no other requests
        }
    }

    /// The offer's receiver vocabulary (Phase 32): `accept` narrows
    /// the MIME the receiver will request (the source learns through
    /// `target`), `receive(mime, fd)` asks the source to serve the
    /// payload into the pipe — the descriptor crosses the request,
    /// the permission gate runs, and the source's `send` event rides
    /// the same descriptor back to its owner. `finish`/`set_actions`
    /// are the DnD negotiation the manager's own machine polices
    /// (a selection offer refuses them with its typed error).
    fn data_offer_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id();
        let mut world = self.shared.world.lock().expect("world lock");
        let Some(key) = world
            .clipboard
            .offer_of_object(data_key(client), request.object)
        else {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(request.object),
                "the object is not a data offer of this client",
            ));
        };
        match request.op.name {
            "accept" => {
                let Value::String(mime) = &request.args[0] else {
                    return Err(bad_shape("data_offer.accept"));
                };
                let batch = world
                    .clipboard
                    .offer_accept(data_key(client), key, mime)
                    .map_err(|e| offer_error(request.object, e))?;
                Self::route_data(ctx, client, &mut world, batch)
            }
            "receive" => {
                let (Value::String(mime), Value::Fd(index)) = (&request.args[0], &request.args[1])
                else {
                    return Err(bad_shape("data_offer.receive"));
                };
                let fd = ctx.take_fd(*index)?;
                // The manifest baseline: same-user peers are unconfined
                // (the sandbox verdict doctrine) — the read scope rides
                // the baseline until the broker deployment narrows it.
                let manifest = ScopeSet::single(Scope::ClipboardRead);
                let (batch, _transfer) = world
                    .clipboard
                    .offer_receive(data_key(client), key, mime, fd, manifest, None)
                    .map_err(|e| receive_error(request.object, &e))?;
                Self::route_data(ctx, client, &mut world, batch)
            }
            "finish" => {
                let batch = world
                    .clipboard
                    .offer_finish(crate::scene::SEAT, data_key(client))
                    .map_err(|e| finish_error(request.object, e))?;
                Self::route_data(ctx, client, &mut world, batch)
            }
            "set_actions" => {
                let Value::Bitset(actions) = &request.args[0] else {
                    return Err(bad_shape("data_offer.set_actions"));
                };
                let set = ldp_clipboard::ActionSet::from_bits(actions.to_words()[0]);
                let batch = world
                    .clipboard
                    .offer_set_actions(crate::scene::SEAT, data_key(client), set)
                    .map_err(|e| dnd_error(request.object, e))?;
                Self::route_data(ctx, client, &mut world, batch)
            }
            _ => Ok(()), // consumed: the offer has no other requests
        }
    }

    /// Route a manager batch: this session's entries stream now (the
    /// objects are live on this connection), everyone else's park in
    /// the outboxes their own dispatchers drain. The `data_offer`
    /// announcements carry their server-chosen object as a
    /// create-first instruction.
    fn route_data(
        ctx: &mut DispatchCtx<'_>,
        me: ClientId,
        world: &mut World,
        routed: Vec<Routed>,
    ) -> Result<()> {
        let mut mine = Vec::new();
        let mut others = Vec::new();
        for r in routed {
            let Some(client) = ClientId::new(u32::try_from(r.client.0).expect("client fits u32"))
            else {
                continue;
            };
            let entry = match r.fd {
                Some(fd) => {
                    OutboxEntry::with_fd(client, r.object, r.event.name(), r.event.args(), fd)
                }
                None => OutboxEntry::event(client, r.object, r.event.name(), r.event.args()),
            };
            // The announcement's new_id becomes a real object in the
            // receiver's store when the entry drains.
            let entry = if let DataEvent::DeviceDataOffer { id } = r.event {
                entry.creating(id, "ldp.data.data_offer")
            } else {
                entry
            };
            if client == me {
                mine.push(entry);
            } else {
                others.push(entry);
            }
        }
        world.outboxes.extend(others);
        Self::emit_entries(ctx, mine)
    }

    fn create_surface(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let Value::NewId(id) = request.args[0] else {
            return Err(bad_shape("compositor.create_surface"));
        };
        ctx.create_object(id, "ldp.core.surface", request.version)?;
        let mut world = self.shared.world.lock().expect("world lock");
        world.scene.create_surface(ctx.client_id(), id);
        Ok(())
    }

    /// `compositor.create_subsurface(surface, parent, role)`.
    fn create_subsurface(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let (surface, parent, id) = match (&request.args[0], &request.args[1], &request.args[2]) {
            (Value::Object(Some(s)), Value::Object(Some(p)), Value::NewId(i)) => (*s, *p, *i),
            _ => return Err(bad_shape("compositor.create_subsurface")),
        };
        let made = {
            let mut world = self.shared.world.lock().expect("world lock");
            world
                .scene
                .create_subsurface(ctx.client_id(), surface, parent, id)
        };
        match made {
            Some(_) => {
                ctx.create_object(id, "ldp.core.subsurface", request.version)?;
                Ok(())
            }
            None => Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(request.object),
                "create_subsurface: unknown surfaces, mapped surface, or role conflict",
            )),
        }
    }

    /// The `ldp.core.surface` family.
    // Phase 48 grew the walk by the close-fade's two triggers (the
    // detach capture, the dialog focus handoff); the per-request
    // dispatch narrative stays one function by doctrine.
    #[allow(clippy::too_many_lines)]
    fn surface_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id();
        let object = request.object;
        let mut world = self.shared.world.lock().expect("world lock");
        let surface = world
            .scene
            .surface_of(client, object)
            .ok_or_else(|| unknown_surface(object))?;
        match request.op.name {
            "attach" => {
                let proposal = Self::surface_attach(&mut world, surface, client, &request.args[0])?;
                drop(world);
                // Phase 36: the popup's placement proposal rides
                // `popup.configure` at attach — the buffer's size is
                // the solver's input (the same doctrine as the
                // toplevel's 0x0 answer).
                if let Some((object, serial, placement)) = proposal {
                    ctx.emit(
                        object,
                        "configure",
                        vec![
                            Value::Uint32(serial),
                            Value::Int32(placement.x),
                            Value::Int32(placement.y),
                            Value::Uint32(placement.width),
                            Value::Uint32(placement.height),
                        ],
                    )?;
                }
                Ok(())
            }
            "damage" => {
                let rects = rect_array(&request.args[0])?;
                world
                    .scene
                    .tree
                    .pending_mut(surface)
                    .map_err(tree_error)?
                    .damage(&rects);
                Ok(())
            }
            "damage_buffer" => {
                let rects = rect_array(&request.args[0])?;
                let current = world
                    .scene
                    .tree
                    .get(surface)
                    .expect("routed surfaces live in the tree")
                    .state()
                    .clone();
                world
                    .scene
                    .tree
                    .pending_mut(surface)
                    .map_err(tree_error)?
                    .damage_buffer(&rects, &current);
                Ok(())
            }
            "commit" => {
                let Value::Uint32(cookie) = request.args[0] else {
                    return Err(bad_shape("surface.commit"));
                };
                let now = world.now();
                // Phase 47: the mapping commit is the WindowOpen
                // motion's start — the first commit that maps the
                // surface begins the compositor-owned fade (the
                // server's own eligibility rules, never the client's:
                // popups and ephemeral roles appear instantly).
                let was_mapped = world
                    .scene
                    .tree
                    .get(surface)
                    .is_some_and(|s| s.state().is_mapped());
                // Phase 48: an unmapping commit (a detach) begins the
                // close fade *before* the tree applies it — the ghost
                // captures the last committed raster while it is
                // still the truth (the route's buffer, the pool's
                // mapping, the node's bounds).
                let unmapping =
                    was_mapped && matches!(world.scene.pending_attach.get(&surface), Some(None));
                if unmapping {
                    world.begin_window_close(surface, now);
                }
                world.scene.commit(client, object, now);
                let now_mapped = world
                    .scene
                    .tree
                    .get(surface)
                    .is_some_and(|s| s.state().is_mapped());
                if !was_mapped && now_mapped {
                    world.begin_window_open(surface, now);
                    // Phase 48: a mapped modal dialog takes the
                    // keyboard from its gated parent (the focus
                    // follows the gate — the parent's keys are
                    // routed nowhere until the dialog closes).
                    if let Some((dc, dobj, parent)) = world.dialogs.by_surface_ids(surface) {
                        if world
                            .dialogs
                            .entry(dc, dobj)
                            .is_some_and(crate::shell::ServedDialog::gates)
                            && world.input.keyboard_focus() == Some(parent)
                        {
                            world.retarget_keyboard_focus(Some(surface));
                        }
                    }
                }
                // Phase 51 — the focus truth: an unmapping commit (a
                // detach) releases the keys the window held — the
                // leave over the wire, the `activated` bit cleared
                // with its proposal, and the frontmost remaining
                // window promoted (the same discipline every
                // focus-holder departure serves; the minimize verb
                // already hid the window at its own dispatch, so its
                // surface never reaches this arm unmapping).
                if was_mapped && !now_mapped && world.input.keyboard_focus() == Some(surface) {
                    world.promote_focus();
                }
                // Phase 49: the toplevel's acked proposal realizes
                // with this commit — the machine's `commit()` returns
                // the applied configure, and its geometry lands now
                // (the buffer the client attached is the size answer;
                // the position completes the placement — a window
                // lands placed, never origin-then-jump).
                world.toplevel_realize(surface);
                drop(world);
                ctx.emit(object, "committed", vec![Value::Uint32(cookie)])
            }
            "frame" => {
                let Value::Uint64(frame) = request.args[0] else {
                    return Err(bad_shape("surface.frame"));
                };
                let now = world.now();
                world.scene.scheduler.frame_request(surface, frame, now);
                Ok(())
            }
            "set_sync_fence" => {
                // v1 has no fence-object factory, so the only reachable
                // argument is null (clear). Explicit-sync gating arrives
                // with the GPU phases.
                Ok(())
            }
            op => Self::surface_pending(&mut world, surface, op, request.args),
        }
    }

    /// `surface.attach(buffer|null)`.
    ///
    /// A popup surface's attach additionally solves its placement
    /// (Phase 36): the returned proposal is the caller's `popup`
    /// `.configure` emission.
    fn surface_attach(
        world: &mut World,
        surface: ldp_compositor::surface::SurfaceId,
        client: ClientId,
        arg: &Value,
    ) -> Result<Option<(ObjectId, u32, ldp_shell::Placement)>> {
        let buffer = match arg {
            Value::Object(None) => None,
            Value::Object(Some(b)) => Some(*b),
            _ => return Err(bad_shape("surface.attach")),
        };
        match buffer {
            None => {
                world.scene.pending_attach.insert(surface, None);
                Ok(None)
            }
            Some(b) => {
                let key = ObjectKey::new(client, b);
                let Some(buf) = world.scene.buffers.get(&key).cloned() else {
                    return Err(LdpError::protocol(
                        ErrorCode::InvalidBuffer,
                        Some(b),
                        "attach: object is not a buffer of this connection",
                    ));
                };
                let size = (buf.width(), buf.height());
                world.scene.pending_attach.insert(surface, Some((b, buf)));
                // Phase 48: a dialog surface's attach centers it over
                // its parent (the spec's server-side centering
                // policy — the buffer's size is the centering input),
                // the position riding the pending queue so the dialog
                // lands placed with the mapping commit. A dialog
                // never takes the toplevel cascade and never solves
                // constraints (it is not a popup): centering is the
                // whole placement.
                if let Some((_, _, parent)) = world.dialogs.by_surface_ids(surface) {
                    let usable = world.shell.usable_physical();
                    let snapshot = world.scene.snapshot();
                    // The parent's content area when it shows one; the
                    // usable area's own center otherwise (an orphan
                    // centering — the parent is between windows).
                    let parent_content = snapshot
                        .node(parent)
                        .filter(|n| n.mapped)
                        .map_or(usable, |n| n.bounds);
                    let (x, y) =
                        ldp_shell::dialog::Dialog::center_over(parent_content, size, usable);
                    world.scene.tree.set_position(surface, x, y).ok();
                    return Ok(None);
                }
                // Phase 36 first: a popup surface's attach solves the
                // placement — the anchor machine's proposal against
                // the usable area (parent coordinates), the position
                // riding the pending queue so the menu lands placed
                // with the mapping commit. A popup never takes the
                // toplevel cascade.
                if let Some((pc, po, parent)) = world.popups.by_surface_ids(surface) {
                    let usable = popup_usable(world, parent);
                    if let Some((serial, placement)) = world.popups.repropose(pc, po, size, usable)
                    {
                        let origin = parent.map_or((0, 0), |p| root_origin(world, p));
                        world
                            .scene
                            .tree
                            .set_position(surface, origin.0 + placement.x, origin.1 + placement.y)
                            .ok();
                        return Ok(Some((ObjectId::from_wire(po), serial, placement)));
                    }
                    return Ok(None);
                }
                // Phase 28: the positioning shell places a root at its
                // first attach — the buffer's size is the placement
                // input (phone: the usable origin above the dock;
                // desktop: the cascade's next step), and the position
                // rides the pending queue so it applies with the very
                // commit that maps the surface: a window lands placed,
                // never origin-then-jump. The legacy default (dock off)
                // keeps creation positions — the byte-exact doctrine.
                // Subsurfaces keep their parent-relative positions; a
                // re-attach of an already placed root does not re-place.
                let is_unplaced_root = world
                    .scene
                    .routes
                    .get(&surface)
                    .is_some_and(|r| r.role_obj.is_none() && !r.placed);
                if world.shell.is_active() && is_unplaced_root {
                    let (x, y) = world.shell.place_root(size);
                    world.scene.tree.set_position(surface, x, y).ok();
                    if let Some(route) = world.scene.routes.get_mut(&surface) {
                        route.placed = true;
                    }
                }
                Ok(None)
            }
        }
    }

    /// The pending-state setters (`set_*` requests).
    fn surface_pending(
        world: &mut World,
        surface: ldp_compositor::surface::SurfaceId,
        op: &str,
        args: &[Value],
    ) -> Result<()> {
        let now = world.now();
        let pending = world.scene.tree.pending_mut(surface).map_err(tree_error)?;
        match op {
            "set_transform" => {
                let t = enum_value(&args[0], "surface.set_transform")?;
                let transform = Transform::from_wire(t).ok_or_else(|| out_of_range(t))?;
                pending.set_transform(transform);
                Ok(())
            }
            "set_buffer_scale" => {
                let Value::Uint32(q8) = args[0] else {
                    return Err(bad_shape("surface.set_buffer_scale"));
                };
                let scale = ScaleFactor::from_q8(q8).ok_or_else(|| out_of_range(q8))?;
                pending.set_buffer_scale(scale);
                Ok(())
            }
            "set_input_region" => {
                let region = region_arg(&args[0])?;
                pending.set_input_region(region);
                Ok(())
            }
            "set_opaque_region" => {
                let region = region_arg(&args[0])?;
                pending.set_opaque_region(region);
                Ok(())
            }
            "set_color" => {
                let color = color_arg(args)?;
                pending.set_color(color);
                Ok(())
            }
            "set_hdr_metadata" => {
                let hdr = hdr_arg(args)?;
                pending.set_hdr_metadata(Some(hdr));
                Ok(())
            }
            "set_presentation_mode" => {
                let m = enum_value(&args[0], "surface.set_presentation_mode")?;
                let mode = PresentationMode::from_wire(m).ok_or_else(|| out_of_range(m))?;
                pending.set_presentation_mode(mode);
                world.scene.scheduler.set_mode(surface, mode, now);
                Ok(())
            }
            _ => Ok(()), // unknown setter: consumed
        }
    }

    /// The `ldp.core.subsurface` family.
    fn subsurface_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id();
        let object = request.object;
        let mut world = self.shared.world.lock().expect("world lock");
        let surface = world
            .scene
            .subsurface_of(client, object)
            .ok_or_else(|| unknown_surface(object))?;
        match request.op.name {
            "set_position" => {
                let (Value::Int32(x), Value::Int32(y)) = (&request.args[0], &request.args[1])
                else {
                    return Err(bad_shape("subsurface.set_position"));
                };
                world
                    .scene
                    .tree
                    .set_position(surface, *x, *y)
                    .map_err(tree_error)?;
                Ok(())
            }
            "place_above" | "place_below" => {
                let Value::Object(Some(sibling)) = &request.args[0] else {
                    return Err(bad_shape("subsurface.place"));
                };
                let Some(sib) = world.scene.surface_of(client, *sibling) else {
                    return Err(unknown_surface(*sibling));
                };
                let placed = if request.op.name == "place_above" {
                    world.scene.tree.place_above(surface, sib)
                } else {
                    world.scene.tree.place_below(surface, sib)
                };
                placed.map_err(tree_error)?;
                Ok(())
            }
            "set_mode" => {
                let m = enum_value(&request.args[0], "subsurface.set_mode")?;
                let mode = match m {
                    1 => SubsurfaceMode::Sync,
                    2 => SubsurfaceMode::Desync,
                    _ => return Err(out_of_range(m)),
                };
                world
                    .scene
                    .tree
                    .set_mode(surface, mode)
                    .map_err(tree_error)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// `capture_manager.grab()` — snapshot the output's current frame.
    ///
    /// The scanout words are copied out under the world lock (between
    /// frames — never torn, never blocking the render loop on client I/O),
    /// written whole into a fresh memfd, and shipped as the `frame` event's
    /// read-once descriptor: the same snapshot vocabulary keymaps and ICC
    /// profiles use, so the remote relay carries captures unchanged. A
    /// memfd or write failure answers `failed(out_of_memory)`; a dark
    /// world answers `failed(no_output)` (Phase 26) — the two honest
    /// failure modes v1 has.
    fn capture_grab(&self, ctx: &mut DispatchCtx<'_>, request: &IncomingRequest<'_>) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let (words, w, h) = {
            let mut world = self.shared.world.lock().expect("world lock");
            // A dark world has no frame to snapshot — the honest
            // failure (`capture_error::no_output`, Phase 26).
            let Some(output) = world.output() else {
                drop(world);
                return ctx.emit(
                    request.object,
                    "failed",
                    vec![Value::Enum(2)], // capture_error::no_output
                );
            };
            let mode = output.mode().clone();
            let mut words = world.scanout_words().unwrap_or_default();
            // Phase 47 — the security-aware capture: surfaces whose
            // effective security class redacts (protected/system, the
            // lock role's floor included) paint black in the
            // client-visible frame. The display keeps showing the
            // content — the compositor controls what becomes *visible*,
            // which is exactly why it is the enforcement point. The
            // unclaimed desktop (the common case) pays nothing: the
            // map is empty and the pass is a single check.
            world.redact_protected(
                &mut words,
                u32::from(mode.hdisplay),
                u32::from(mode.vdisplay),
            );
            (words, u32::from(mode.hdisplay), u32::from(mode.vdisplay))
        };
        let snapshot: Option<std::fs::File> = (|| {
            let fd = crate::sys::memfd("ldp-capture").ok()?;
            let mut file = std::fs::File::from(fd);
            let mut bytes = Vec::with_capacity(words.len() * 4);
            for word in &words {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            file.write_all(&bytes).ok()?;
            // The descriptor shares its offset with the receiving
            // duplicate: rewind so the client reads from the start.
            file.seek(SeekFrom::Start(0)).ok()?;
            Some(file)
        })();
        match snapshot {
            Some(file) => {
                let mut fds = FdList::new();
                fds.push(file.into());
                ctx.emit_fd(
                    request.object,
                    "frame",
                    vec![Value::Fd(0), Value::Uint32(w), Value::Uint32(h)],
                    &mut fds,
                )
            }
            None => ctx.emit(
                request.object,
                "failed",
                vec![Value::Enum(1)], // capture_error::out_of_memory
            ),
        }
    }

    /// `shm.create_pool(fd, size, id)`.
    fn shm_create_pool(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let (Value::Fd(index), Value::Int64(size), Value::NewId(id)) =
            (&request.args[0], &request.args[1], &request.args[2])
        else {
            return Err(bad_shape("shm.create_pool"));
        };
        if *size < 0 {
            return Err(LdpError::protocol(
                ErrorCode::OutOfRange,
                Some(request.object),
                "create_pool: negative size",
            ));
        }
        let fd = ctx.take_fd(*index)?;
        let pool = ShmPool::new(fd, u64::try_from(*size).unwrap_or(u64::MAX))
            .map_err(|e| shm_error(request.object, e))?;
        ctx.create_object(*id, "ldp.core.shm_pool", request.version)?;
        let mut world = self.shared.world.lock().expect("world lock");
        world
            .scene
            .pools
            .insert(ObjectKey::new(ctx.client_id(), *id), pool);
        Ok(())
    }

    /// The `ldp.core.shm_pool` family.
    fn shm_pool_request(
        &self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        let client = ctx.client_id();
        let object = request.object;
        let mut world = self.shared.world.lock().expect("world lock");
        let key = ObjectKey::new(client, object);
        match request.op.name {
            "create_buffer" => {
                let (
                    Value::Int32(offset),
                    Value::Int32(w),
                    Value::Int32(h),
                    Value::Int32(stride),
                    Value::Uint32(format),
                    Value::NewId(id),
                ) = (
                    &request.args[0],
                    &request.args[1],
                    &request.args[2],
                    &request.args[3],
                    &request.args[4],
                    &request.args[5],
                )
                else {
                    return Err(bad_shape("shm_pool.create_buffer"));
                };
                let pool_size = world.scene.pools.get(&key).map_or(0, ShmPool::size);
                let pool_ref = ObjectKeyRef::new(key.client, key.object);
                let buffer = ShmBuffer::new(pool_ref, pool_size, *offset, *w, *h, *stride, *format)
                    .map_err(|e| shm_error(object, e))?;
                drop(world);
                ctx.create_object(*id, "ldp.core.buffer", request.version)?;
                let mut world = self.shared.world.lock().expect("world lock");
                world
                    .scene
                    .buffers
                    .insert(ObjectKey::new(client, *id), Arc::new(buffer));
                Ok(())
            }
            "resize" => {
                let Value::Int64(size) = &request.args[0] else {
                    return Err(bad_shape("shm_pool.resize"));
                };
                let pool = world
                    .scene
                    .pools
                    .get_mut(&key)
                    .ok_or_else(|| unknown_pool(object))?;
                pool.resize(u64::try_from((*size).max(0)).unwrap_or(u64::MAX))
                    .map_err(|e| shm_error(object, e))?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl Dispatcher for CompositorDispatcher {
    fn on_request(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        match (request.interface, request.op.name) {
            ("ldp.core.compositor", "create_surface") => self.create_surface(ctx, request),
            ("ldp.core.compositor", "create_subsurface") => self.create_subsurface(ctx, request),
            ("ldp.core.surface", _) => self.surface_request(ctx, request),
            ("ldp.core.subsurface", _) => self.subsurface_request(ctx, request),
            ("ldp.core.shm", "create_pool") => self.shm_create_pool(ctx, request),
            ("ldp.core.shm_pool", _) => self.shm_pool_request(ctx, request),
            ("ldp.capture.capture_manager", "grab") => self.capture_grab(ctx, request),
            ("ldp.input.seat", _) => self.seat_request(ctx, request),
            ("ldp.shell.shell", _) => self.shell_request(ctx, request),
            ("ldp.shell.toplevel", _) => self.toplevel_request(ctx, request),
            ("ldp.shell.popup", _) => self.popup_request(ctx, request),
            ("ldp.shell.dialog", _) => self.dialog_request(ctx, request),
            ("ldp.data.data_device_manager", _) => self.data_manager_request(ctx, request),
            ("ldp.data.data_source", _) => self.data_source_request(ctx, request),
            ("ldp.data.data_device", _) => self.data_device_request(ctx, request),
            ("ldp.data.data_offer", _) => self.data_offer_request(ctx, request),
            _ => Ok(()), // interfaces without dispatcher state: consumed
        }
    }

    fn on_bind(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        object: ObjectId,
        interface: &str,
        _version: u32,
    ) -> Result<()> {
        let client = ctx.client_id();
        {
            let mut world = self.shared.world.lock().expect("world lock");
            // Phase 44: the bind joins the withdrawal sweep's candidate
            // set (unless it is revoked immediately below — a revoked
            // object is not a live candidate).
            world.track_global_bind(client, object, interface);
        }
        // Phase 44: a withdrawn interface is accepted by the static
        // server config and answered here with the immediate
        // revocation the dark state made its own — the client learns
        // "not available right now" at the bind, exactly as a dark
        // output teaches it. The registry already said so
        // (`global_remove`); this closes the race a client that
        // missed the barrier would otherwise ride.
        {
            let withdrawn = {
                let world = self.shared.world.lock().expect("world lock");
                world.withdrawn_globals.contains(interface)
            };
            if withdrawn {
                let reason =
                    ldp_protocol::generated::core::RevokedReason::InterfaceRemoved.to_wire();
                let entry = OutboxEntry::revoke(client, object, reason);
                let mut world = self.shared.world.lock().expect("world lock");
                world.untrack_global_bind(object);
                drop(world);
                return Self::emit_entries(ctx, vec![entry]);
            }
        }
        match interface {
            "ldp.core.output" => {
                // The dark state: the bind succeeds (the global is
                // advertised), but there is no output behind it — the
                // honest answer is an immediate revocation, so the
                // client learns "no display right now" instead of
                // waiting for a cascade that cannot come.
                //
                // Phase 31's round-robin: a client binds
                // `ldp.core.output` once per display it cares about —
                // each bind mirrors the *next* served output (slot
                // order), wrapping; a single-output world hands every
                // bind the one output, the Phase 25 behavior.
                let cascade = {
                    let mut world = self.shared.world.lock().expect("world lock");
                    if world.outputs.is_empty() {
                        Vec::new()
                    } else {
                        let seq = world
                            .scene
                            .output_bind_seq
                            .entry(client.as_u32())
                            .and_modify(|n| *n = n.wrapping_add(1))
                            .or_insert(0);
                        let slot_index = (*seq as usize) % world.outputs.len();
                        let slot_crtc = world.outputs[slot_index].crtc;
                        let cascade = world.outputs[slot_index].output.cascade();
                        world
                            .scene
                            .output_binds
                            .insert((client.as_u32(), slot_crtc), object);
                        cascade
                    }
                };
                if cascade.is_empty() {
                    // A revoked object is not a live withdrawal
                    // candidate — untrack it before the revocation.
                    {
                        let mut world = self.shared.world.lock().expect("world lock");
                        world.untrack_global_bind(object);
                    }
                    let reason =
                        ldp_protocol::generated::core::RevokedReason::CapabilityRevoked.to_wire();
                    let entry = OutboxEntry::revoke(client, object, reason);
                    return Self::emit_entries(ctx, vec![entry]);
                }
                for (event, args) in cascade {
                    ctx.emit(object, event, args)?;
                }
                Ok(())
            }
            "ldp.core.shm" => {
                for format in SHM_FORMATS {
                    ctx.emit(object, "format", vec![Value::Uint32(format.code())])?;
                }
                Ok(())
            }
            "ldp.shell.shell" => {
                // Phase 49: the shell bind answers with the spaces
                // count (the spec's "after binding") — the states
                // arm's first bind-time event, the client's license
                // to move windows (`set_workspace` clamps into it).
                // Phase 51: the binder joins the view switch's
                // broadcast set (`workspace_switched` finds it here).
                let count = {
                    let mut world = self.shared.world.lock().expect("world lock");
                    world.track_shell_bind(client, object);
                    world.spaces.count()
                };
                ctx.emit(object, "workspace_count", vec![Value::Uint32(count)])
            }
            _ => Ok(()),
        }
    }

    fn on_registry(&mut self, ctx: &mut DispatchCtx<'_>, object: ObjectId) -> Result<()> {
        // Phase 44: the registry becomes a fan-out target for the
        // dynamic advertisement set — `global` / `global_remove`
        // sweeps emit on exactly these.
        let client = ctx.client_id();
        let mut world = self.shared.world.lock().expect("world lock");
        world.track_registry(client, object);
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // every interface's death sweep, one narrative
    fn on_destroy(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        object: ObjectId,
        interface: &str,
    ) -> Result<()> {
        let client = ctx.client_id();
        let mut world = self.shared.world.lock().expect("world lock");
        // Phase 44: a destroyed object leaves the dynamic-globals
        // bookkeeping whatever it was (a registry stops being a
        // fan-out target; a global bind stops being a withdrawal
        // candidate). Phase 51: a destroyed shell bind leaves the
        // view switch's broadcast set the same way.
        world.untrack_registry(object);
        world.untrack_global_bind(object);
        world.untrack_shell_bind(object);
        // Popups the destroyed object orphans (parent or own surface
        // death): dismissed with `done` — a menu cannot outlive its
        // window, and a roleless popup surface has nothing to show.
        let mut orphaned: Vec<ObjectId> = Vec::new();
        // Dialogs the destroyed object closes (the dialog's own
        // surface dying, or its parent's): `close` tells the client
        // the dialog is over (Phase 48 — a dialog cannot outlive its
        // window, the sheet-dies-with-window doctrine).
        let mut closed_dialogs: Vec<ObjectId> = Vec::new();
        match interface {
            "ldp.core.surface" => {
                if let Some(surface) = world.scene.surface_of(client, object) {
                    // Phase 48: the window leaving the desktop fades
                    // out — the ghost captures the last committed
                    // raster *before* anything eats the truth (the
                    // popup dismissal below would end the popup
                    // machine, and the destroy itself eats the route
                    // and the node). The server's own eligibility
                    // rules run inside: a popup surface still carries
                    // its live role here and never ghosts, and with
                    // transitions off this is a no-op.
                    let now = world.now();
                    world.begin_window_close(surface, now);
                    // The popup's own surface dying ends the popup.
                    if world.popups.is_popup_surface(surface) {
                        if let Some(entry) = world.popups.by_surface_mut(surface) {
                            if entry.machine.is_live() {
                                entry.machine.dismiss();
                            }
                            orphaned.push(ObjectId::from_wire(entry.object));
                        }
                    }
                    // Children of the dying parent cannot stay.
                    for (pc, po) in world.popups.children_of_ids(surface) {
                        if let Some(entry) = world.popups.entry_mut(pc, po) {
                            if entry.machine.is_live() {
                                entry.machine.dismiss();
                            }
                            orphaned.push(ObjectId::from_wire(po));
                        }
                    }
                    // Phase 48: the dialog sweeps — the dying surface's
                    // own dialogs close, and so do every dialog the
                    // dying *parent* owns (a dialog cannot outlive
                    // its window). Only the freshly closed emit (a
                    // close is idempotent, never a double event).
                    for (_, dobj) in world.dialogs.close_surface(surface) {
                        closed_dialogs.push(ObjectId::from_wire(dobj));
                    }
                    for (_, dobj) in world.dialogs.close_children(surface) {
                        closed_dialogs.push(ObjectId::from_wire(dobj));
                    }
                    // Phase 49: the states arm's death sweep — the
                    // toplevel entries, the spaces membership, and
                    // the hidden-set membership die with the window
                    // (a destroyed workspace resident leaves no
                    // residue; nothing is emitted — the client's own
                    // surface death already told it).
                    world.toplevels.drop_surface(surface);
                    world
                        .spaces
                        .remove_window(ldp_shell::WindowKey::new(surface.raw()));
                    world.scene.hidden.remove(&surface);
                    // Phase 50: a drag on the dying window ends with
                    // it (the grip holds nothing now; the drag's own
                    // machine is gone with the entry above — nothing
                    // to clear, nothing to emit).
                    if world.drags.surface() == Some(surface) {
                        world.end_drag_silent();
                    }
                    world.scene.destroy_surface(surface);
                    // Phase 51 — the focus truth: a dying focus holder
                    // hands the keys to the frontmost window that
                    // remains (the desktop's own truth). The dead
                    // surface's own leave and proposal naturally emit
                    // nothing — its route and its machine entry are
                    // already gone.
                    if world.input.keyboard_focus() == Some(surface) {
                        world.promote_focus();
                    }
                }
            }
            "ldp.shell.popup" => {
                // The role object dies: the host drops the entry (the
                // surface may outlive it roleless, or die with it).
                world.popups.drop_entry(client.as_u32(), object.as_u32());
            }
            "ldp.shell.toplevel" => {
                // Phase 49: the role object dies — the host drops the
                // entry (the machine and its states die with the
                // object), and the scene releases the object mapping
                // (the surface may outlive the role, roleless — a
                // re-mint replaces both mappings). Nothing is
                // emitted: the client's own destroy already told it.
                // Phase 50: a live drag on the role object ends with
                // it (the drag's machine and proposals belong to the
                // dying object; the surface may live on, the grip
                // does not).
                if world
                    .drags
                    .live()
                    .is_some_and(|d| d.client == client.as_u32() && d.object == object.as_u32())
                {
                    world.end_drag_silent();
                }
                world.toplevels.drop_entry(client.as_u32(), object.as_u32());
                if let Some(surface) = world.scene.toplevel_of(client, object) {
                    world.scene.unregister_toplevel(client, object, surface);
                }
            }
            "ldp.shell.dialog" => {
                // The dialog object dies: the host drops the entry
                // (the surface may outlive it roleless, or die with
                // it — the client's teardown, nothing to tell it).
                world.dialogs.drop_entry(client.as_u32(), object.as_u32());
            }
            "ldp.core.subsurface" => {
                if let Some(surface) = world.scene.subsurface_of(client, object) {
                    world.scene.destroy_surface(surface);
                }
            }
            "ldp.core.shm_pool" => {
                world.scene.pools.remove(&ObjectKey::new(client, object));
            }
            "ldp.core.buffer" => {
                world.scene.buffers.remove(&ObjectKey::new(client, object));
            }
            "ldp.core.output" => {
                world
                    .scene
                    .output_binds
                    .retain(|(_, _), bound| *bound != object);
            }
            "ldp.data.data_source" => {
                // The source dies: its slot ownership (if any) clears
                // with null announcements to the receivers, its
                // transfers cancel, and an evicted previous owner's
                // `cancelled` routes to whoever still holds one.
                if let Some(key) = world.clipboard.source_of_object(data_key(client), object) {
                    let routed = world.clipboard.source_destroy(key);
                    return Self::route_data(ctx, client, &mut world, routed);
                }
            }
            "ldp.data.data_offer" => {
                let routed = world.clipboard.offer_destroy(data_key(client), object);
                return Self::route_data(ctx, client, &mut world, routed);
            }
            "ldp.data.data_device" => {
                // The device object dies with its client's teardown
                // (connection destroy or session end); the manager's
                // device registry drops it there. Nothing routes.
                drop(world);
                return Ok(());
            }
            _ => {}
        }
        // The orphaned popups' `done` — emitted after the lock drops
        // (the spec: the client stops rendering it).
        drop(world);
        for target in orphaned {
            ctx.emit(target, "done", vec![])?;
        }
        // The closed dialogs' `close` — after the lock drops (the
        // spec: the user asked to close; the client destroys the
        // dialog when ready).
        for target in closed_dialogs {
            ctx.emit(target, "close", vec![])?;
        }
        Ok(())
    }

    fn on_wake(&mut self, ctx: &mut DispatchCtx<'_>) -> Result<()> {
        let client = ctx.client_id();
        let (parked, fresh) = {
            let mut world = self.shared.world.lock().expect("world lock");
            // Every client message is interaction (the idle ladder's
            // feed — Phase 31); a blanked world lights on the wake.
            if let Err(e) = world.note_activity() {
                return Err(pump_failure(&e));
            }
            // Older events parked by other clients' pumps stream first.
            let parked = world.outboxes.drain(client);
            let fresh = world.pump(client).map_err(|e| pump_failure(&e))?;
            (parked, fresh)
        };
        Self::emit_entries(ctx, parked)?;
        Self::emit_entries(ctx, fresh)
    }

    fn on_session_end(
        &mut self,
        client: ClientId,
        _reason: &SessionEnd,
        _objects: &[(ObjectId, ObjectEntry)],
    ) {
        let mut world = self.shared.world.lock().expect("world lock");
        // The data family first: the teardown routes (null selection
        // announcements to the survivors) before the scene forgets
        // the client — the routed events reference objects only the
        // surviving sessions hold. There is no session context to
        // emit through anymore: everything parks in the outboxes,
        // and the dead client's own queue drops with the rest below.
        let routed = world.clipboard.client_gone(data_key(client));
        for r in routed {
            if let Some(entry) = routed_entry(r) {
                world.outboxes.push(entry);
            }
        }
        // The input binding drops with the client (the router's grabs
        // dismiss through the surface-gone reflex at the scene drop).
        world.input.client_gone(client);
        // Phase 50: the client's grip ends with its session (the
        // drag's machine and its outbox target die with it).
        if world
            .drags
            .live()
            .is_some_and(|d| d.client == client.as_u32())
        {
            world.end_drag_silent();
        }
        world.scene.drop_client(client);
        // Phase 44 bookkeeping: the gone client's registries (fan-out
        // targets) and global binds (withdrawal candidates) leave with
        // it — dead sessions never receive global_remove, and their
        // objects are never revoked.
        world.live_registries.retain(|_, c| *c != client);
        world.bound_globals.retain(|_, (c, _)| *c != client);
        // Phase 51: the gone client's shell binds leave the view
        // switch's broadcast set (a dead session never learns a
        // space moved).
        world.shell_binds.retain(|_, c| *c != client);
        world.outboxes.drop_client(client);
    }
}

/// A pump failure rendered as the session-fatal I/O arm.
fn pump_failure(e: &crate::frame_loop::FrameError) -> LdpError {
    LdpError::Io(Arc::new(std::io::Error::other(e.to_string())))
}

// ---- argument decoding helpers -------------------------------------------

fn bad_shape(op: &str) -> LdpError {
    LdpError::protocol(
        ErrorCode::SignatureMismatch,
        None,
        format!("{op}: argument failed its schema shape"),
    )
}

fn unknown_surface(object: ObjectId) -> LdpError {
    LdpError::protocol(
        ErrorCode::InvalidObject,
        Some(object),
        "object does not route to a live surface in this compositor",
    )
}

/// An object that routes nowhere (the popup host's lookup miss).
fn unknown_object(object: ObjectId) -> LdpError {
    LdpError::protocol(
        ErrorCode::InvalidObject,
        Some(object),
        "object does not route to a live role in this compositor",
    )
}

fn unknown_pool(object: ObjectId) -> LdpError {
    LdpError::protocol(
        ErrorCode::InvalidObject,
        Some(object),
        "object is not a shm pool of this connection",
    )
}

fn out_of_range(v: u32) -> LdpError {
    LdpError::protocol(
        ErrorCode::OutOfRange,
        None,
        format!("value {v} is outside the declared domain"),
    )
}

fn tree_error(e: ldp_compositor::tree::TreeError) -> LdpError {
    LdpError::protocol(
        ErrorCode::InvalidState,
        None,
        format!("surface tree rejected the operation: {e}"),
    )
}

fn shm_error(object: ObjectId, e: ShmError) -> LdpError {
    let code = match e {
        ShmError::Shrink => ErrorCode::InvalidState,
        _ => ErrorCode::InvalidBuffer,
    };
    LdpError::protocol(code, Some(object), e.to_string())
}

fn enum_value(arg: &Value, op: &str) -> Result<u32> {
    match arg {
        Value::Enum(v) => Ok(*v),
        _ => Err(bad_shape(op)),
    }
}

/// The popup solver's usable area, expressed in *parent coordinates*
/// (the frame the anchor geometry lives in): the shell's physical
/// usable carve translated by the parent's absolute origin (a null
/// parent's anchor already lives on the output — no translation).
fn popup_usable(world: &World, parent: Option<ldp_compositor::surface::SurfaceId>) -> Rect {
    let usable = world.shell.usable_physical();
    match parent {
        None => usable,
        Some(p) => {
            let (px, py) = root_origin(world, p);
            Rect::new(usable.x - px, usable.y - py, usable.w, usable.h)
        }
    }
}

/// A root surface's absolute (output-space) position — the popup's
/// parent translation. Roots carry their position directly (no
/// ancestor chain); the tree's accessor is the truth.
fn root_origin(world: &World, surface: ldp_compositor::surface::SurfaceId) -> (i32, i32) {
    world
        .scene
        .tree
        .get(surface)
        .map_or((0, 0), Surface::position)
}

/// Decode an `array<rect>` argument.
fn rect_array(arg: &Value) -> Result<Vec<Rect>> {
    match arg {
        Value::Array { element, items } if *element == ArgType::Rect => items
            .iter()
            .map(|p| match p {
                ldp_core::wire::Primitive::Rect(r) => Ok(*r),
                _ => Err(bad_shape("rect array element")),
            })
            .collect(),
        _ => Err(bad_shape("rect array")),
    }
}

/// Build a region from an `array<rect>` argument.
fn region_arg(arg: &Value) -> Result<Region> {
    let rects = rect_array(arg)?;
    let mut region = Region::new();
    for r in rects {
        region.add_region(&Region::from_rect(r));
    }
    Ok(region)
}

/// Decode `set_color`'s six arguments.
fn color_arg(args: &[Value]) -> Result<ColorDescription> {
    let primaries = Primaries::from_wire(enum_value(&args[0], "set_color")?)
        .ok_or_else(|| out_of_range_enum(&args[0], "primaries"))?;
    let transfer = TransferFunction::from_wire(enum_value(&args[1], "set_color")?)
        .ok_or_else(|| out_of_range_enum(&args[1], "transfer"))?;
    let range = ColorRange::from_wire(enum_value(&args[2], "set_color")?)
        .ok_or_else(|| out_of_range_enum(&args[2], "range"))?;
    let nums: [u32; 3] = match (&args[3], &args[4], &args[5]) {
        (Value::Uint32(a), Value::Uint32(b), Value::Uint32(c)) => [*a, *b, *c],
        _ => return Err(bad_shape("set_color")),
    };
    Ok(ColorDescription {
        primaries,
        transfer,
        range,
        luminance_min: Luminance::from_wire_units(nums[0]),
        luminance_max: Luminance::from_wire_units(nums[1]),
        reference_white: Luminance::from_wire_units(nums[2]),
    })
}

/// Decode `set_hdr_metadata`'s five arguments.
fn hdr_arg(args: &[Value]) -> Result<HdrMetadata> {
    let primaries = Primaries::from_wire(enum_value(&args[0], "set_hdr_metadata")?)
        .ok_or_else(|| out_of_range_enum(&args[0], "primaries"))?;
    let nums: [u32; 4] = match (&args[1], &args[2], &args[3], &args[4]) {
        (Value::Uint32(a), Value::Uint32(b), Value::Uint32(c), Value::Uint32(d)) => {
            [*a, *b, *c, *d]
        }
        _ => return Err(bad_shape("set_hdr_metadata")),
    };
    Ok(HdrMetadata {
        mastering_primaries: primaries,
        mastering_luminance_min: Luminance::from_wire_units(nums[0]),
        mastering_luminance_max: Luminance::from_wire_units(nums[1]),
        max_cll: Luminance::from_wire_units(nums[2]),
        max_fall: Luminance::from_wire_units(nums[3]),
    })
}

fn out_of_range_enum(arg: &Value, what: &str) -> LdpError {
    let v = match arg {
        Value::Enum(v) => *v,
        _ => 0,
    };
    LdpError::protocol(
        ErrorCode::OutOfRange,
        None,
        format!("{what} value {v} is outside the declared domain"),
    )
}

// ---- the data family's dispatch plumbing (Phase 32) -----------------

/// The clipboard manager's client identity: the session's
/// [`ClientId`] widened to the manager's u64 key (every session is a
/// client; the manager never sees a key the compositor did not mint).
fn data_key(client: ClientId) -> ldp_clipboard::ClientKey {
    ldp_clipboard::ClientKey(u64::from(client.as_u32()))
}

/// One routed manager event as an outbox entry (the cross-client
/// delivery vehicle): the event's name and wire arguments verbatim,
/// the pipe descriptor riding the `send` event, and the
/// `data_offer` announcement carrying its server-chosen object as a
/// create-first instruction for the receiver's store.
fn routed_entry(r: Routed) -> Option<OutboxEntry> {
    let client = ClientId::new(u32::try_from(r.client.0).expect("client fits u32"))?;
    let entry = match r.fd {
        Some(fd) => OutboxEntry::with_fd(client, r.object, r.event.name(), r.event.args(), fd),
        None => OutboxEntry::event(client, r.object, r.event.name(), r.event.args()),
    };
    let entry = if let DataEvent::DeviceDataOffer { id } = r.event {
        entry.creating(id, "ldp.data.data_offer")
    } else {
        entry
    };
    Some(entry)
}

/// The data family's error taxonomy arms (the manager's typed errors
/// rendered as the protocol's stable codes).
fn source_error(object: ObjectId, e: SourceError) -> LdpError {
    let code = match e {
        SourceError::OfferTooLate | SourceError::Dead | SourceError::AlreadyAttached => {
            ErrorCode::InvalidState
        }
        SourceError::TooManyOffers => ErrorCode::LimitExceeded,
        SourceError::BadMime => ErrorCode::OutOfRange,
    };
    LdpError::protocol(code, Some(object), e.to_string())
}

/// The device family's arms: the serial freshness and ownership rules
/// are state conditions; an unknown source is the object's own truth.
fn device_error(object: ObjectId, e: DeviceError) -> LdpError {
    let code = match e {
        DeviceError::BadSerial
        | DeviceError::ForeignSource
        | DeviceError::NotOwner
        | DeviceError::SourceUnavailable(_) => ErrorCode::InvalidState,
        DeviceError::UnknownSource => ErrorCode::InvalidObject,
    };
    LdpError::protocol(code, Some(object), e.to_string())
}

/// The offer family's arms: an unoffered or malformed MIME is a value
/// outside the offer's declared domain; the state machine's refusals
/// are state conditions.
fn offer_error(object: ObjectId, e: OfferError) -> LdpError {
    let code = match e {
        OfferError::BadMime | OfferError::NotOffered => ErrorCode::OutOfRange,
        OfferError::AlreadyReceived
        | OfferError::NotDrag
        | OfferError::NotDropped
        | OfferError::Dead
        | OfferError::TransferRejected => ErrorCode::InvalidState,
    };
    LdpError::protocol(code, Some(object), e.to_string())
}

/// The receive gate's arms: the permission denial is `unauthorized`
/// (the audit trail carries the record), the admission ceiling is the
/// limits arm.
fn receive_error(object: ObjectId, e: &ReceiveError) -> LdpError {
    let code = match e {
        ReceiveError::Denied(_) => ErrorCode::Unauthorized,
        ReceiveError::Offer(offer) => return offer_error(object, *offer),
        ReceiveError::UnknownOffer => ErrorCode::InvalidObject,
        ReceiveError::Admission(AdmissionError::FdBudget) => ErrorCode::LimitExceeded,
        ReceiveError::Admission(AdmissionError::Duplicate) => ErrorCode::InvalidState,
    };
    LdpError::protocol(code, Some(object), e.to_string())
}

/// The DnD finish arms (the machine's refusals are state conditions).
fn finish_error(object: ObjectId, e: FinishError) -> LdpError {
    match e {
        FinishError::Offer(offer) => offer_error(object, offer),
        FinishError::Machine(m) => dnd_error(object, m),
    }
}

/// The DnD negotiation arms: the machine's refusals (serial,
/// phase, receiver identity) are all state conditions — one code,
/// the message carries the machine's own words.
fn dnd_error(object: ObjectId, e: DndError) -> LdpError {
    LdpError::protocol(ErrorCode::InvalidState, Some(object), e.to_string())
}
