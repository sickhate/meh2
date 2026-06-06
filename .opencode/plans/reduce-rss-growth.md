# Reduce RSS Memory Growth Plan

Status: **complete** (2026-06 audit)

## Completed

- Cache `var_refs` in Binding/LoopBinding — computed once at build time
- Skip bindings whose vars didn't change (`intersects()` + changed-var set)
- Binding update builds minimal var map (only referenced vars, not full global_vars clone)
- Popup teardown: `gtk_window_destroy()` + `malloc_trim(0)` on Linux glibc
- Poll output deduplication in script-vars
- Rhai AST cache (`Arc<AST>`)
- Shared `Arc<HashMap>` for widget definitions — loop/rhai bindings no longer clone the full defwidget table per binding
- `VarState::set` returns bool — skip binding updates when value unchanged
- deflisten gating — shell and Rhai listen vars pause subprocess/tick when no windows are open (same model as defpoll)
- Plugin poll dedup — plugin vars skip channel send when output unchanged
- Tokio runtime — 1 worker thread + 8 blocking threads; explicit feature set (not `full`)
- Systray icon pixmap cap — 256×256 max
- IPC max message size — 16 MB
- gtk4-impl split — bindings / builder / widgets / runtime modules

## Rejected

- mimalloc global allocator — masked the real leak; reverted in favor of proper GTK destroy + malloc_trim

## User config (optional)

- Cap notification history in user scripts (e.g. `MAX_NOTIFS=50` in notifications.sh)

## Environment tips

- `GSK_RENDERER=cairo` — ~28 MB RSS vs ~50 MB with default Vulkan renderer for static bars
