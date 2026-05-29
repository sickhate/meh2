# Reduce RSS Memory Growth Plan

## Changes

### 1. Add mimalloc global allocator (`crates/cli/src/main.rs`)
- `#[global_allocator] static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;`

### 2. Cache `var_refs` in Binding/LoopBinding (`crates/gtk4-impl/src/lib.rs`)
- Add `var_refs: Vec<VarName>` field, computed once at build time

### 3. Skip bindings whose vars didn't change (`app.rs` + `lib.rs`)
- Add `intersects()` method to AnyBinding, check against changed var set

### 4. Cap notification history (`notifications.sh`)
- MAX_NOTIFS=50

### 5. Add mimalloc dep to cli Cargo.toml

## Files to Modify
- `Cargo.toml` ✅ already done
- `crates/cli/Cargo.toml` — add mimalloc
- `crates/cli/src/main.rs` — global_allocator
- `crates/gtk4-impl/src/lib.rs` — var_refs + intersects
- `crates/gtk4-impl/src/app.rs` — update_bindings changed_vars
- `~/.config/meh2/scripts/notifications.sh` — MAX_NOTIFS cap
