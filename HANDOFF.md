# ShadowTaskbar Rust prototype — handoff

## Status: COMPILES CLEAN (release), NOT YET RUN

Built successfully on a second machine (2026-10-07) with `rustc`/`cargo`
1.98.1 and `windows = "0.58"`. Release binary is 112 KB at
`target/release/shadowtaskbar-rs.exe`. It has not been launched/exercised yet
— only `cargo build --release` has been verified.

### The linker problem, resolved

Same symptom as described below (`C:\Program Files\coreutils\bin\link.exe`
shadowing the real linker), but this machine *does* have a real MSVC
`link.exe` — a VS "BuildTools" install with the C++ workload
(`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`), just not ahead of
coreutils in PATH. Fix used: prepend the real linker's directory to PATH for
the build invocation, e.g. (PowerShell):

```powershell
$msvcBin = "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\<version>\bin\Hostx64\x64"
$env:PATH = "$msvcBin;$env:PATH"
cargo build --release
```

Find `<version>` with
`Get-ChildItem "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC" -Directory`.
If no VS install has the C++ workload at all, the mingw/gnu-toolchain
fallback further down still applies.

### windows-rs 0.58 API mismatches found by the compiler, now fixed

Once linking worked, `cargo build` got past linking into real type-checking
and surfaced exactly the kind of drift anticipated below. Three fixes applied
in `src/main.rs` / `Cargo.toml`:

1. `CreateMutexW` was an unresolved import — it's gated behind the
   `Win32_Security` Cargo feature in 0.58 (not just `Win32_System_Threading`).
   Added `"Win32_Security"` to the `windows` feature list in `Cargo.toml`.
2. `Some(hinstance.into())` passed to `CreateWindowExW`'s `hInstance` param —
   wrapping in `Option`/`Some` broke the crate's generic `Param<T>` trait
   resolution (it wants `HMODULE` passed directly, since `HMODULE: CanInto<HINSTANCE>`
   already covers the `Option`-free case). Changed to pass `hinstance` directly,
   no `.into()`/`Some(...)` needed.
3. Same issue in the `ABN_FULLSCREENAPP` handler: `SetWindowPos(hwnd, Some(target), ...)`
   — `target: HWND` needs to be passed bare, not wrapped in `Some(...)`.
   Changed to `SetWindowPos(hwnd, target, ...)`.

General pattern across all three: in this version of `windows-rs`, wrapping an
already-concrete handle value in `Some(...)` for a `Param<T>`-generic
parameter breaks trait resolution (`Option<HWND>`/`Option<HINSTANCE>` don't
implement `Param` directly, only the bare handle type does, or literal `None`
does). The compiler error to look for is
`type mismatch resolving <Option<X> as TypeKind>::TypeKind == CopyType` —
the fix is almost always "stop wrapping it in `Some(...)`".

`ABE_BOTTOM` (concern #1 below) and `HBRUSH(GetStockObject(...).0)` (concern
#3) both typechecked as originally written — no changes needed there.

## Original uncompiled note (kept for history)

This was written on a different machine with Rust installed but **no working
linker** (no MSVC `link.exe`, no clang/lld, no mingw gcc — Visual Studio
present but without the "Desktop development with C++" workload, and
`C:\Program Files\coreutils\bin\link.exe` shadows the real linker in PATH
regardless). `cargo check` failed at the build-script-linking stage before any
of this crate's own code was even type-checked, so none of the code had been
compiled or run at that point.

## What it does

Re-implements the core of this repo's `AppBarWindow.cs` in ~250 lines of Rust
using the `windows` crate (0.58) instead of WPF/.NET:

- Registers a Win32 appbar (`SHAppBarMessage` / `ABM_NEW`) docked to the
  bottom edge of the primary monitor, reserving screen space the same way
  the taskbar does (`ABM_QUERYPOS` + `ABM_SETPOS` dance, mirrored from
  `src/MainApp/AppBar/AppBarWindow.cs`'s `AppBarUpdate()`).
- Paints the window solid black via the window class's stock black brush —
  no WPF, no XAML, no rendering per frame.
- Single-instance via a named global mutex (`CreateMutexW` +
  `ERROR_ALREADY_EXISTS` check) — simpler than .NET's approach, no IPC needed.
- `WS_EX_TOOLWINDOW` set at window creation so it never appears in the
  taskbar/alt-tab (equivalent of `WndAndApp.HideFromTaskbar`).
- Handles `ABN_POSCHANGED` (taskbar moved/resized/hidden → recompute) and
  `ABN_FULLSCREENAPP` (push to bottom/top of z-order), and blocks
  `WM_WINDOWPOSCHANGING` from anything other than our own resize, matching
  the C# `WndProc` logic.
- Bar height defaults to 48px, overridable via first CLI arg, e.g.
  `shadowtaskbar-rs.exe 40`.

Left out on purpose (prototype, not feature parity): multi-monitor support
(uses `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`, i.e. primary monitor only),
configurable dock edge (hardcoded to bottom), settings persistence, DPI
per-monitor re-layout beyond the initial awareness call.

## How to build

```
cd shadowtaskbar-rs
cargo build --release
```

Needs a working MSVC linker (`link.exe` from "C++ build tools"/"Desktop
development with C++" in Visual Studio Installer) **or** switch the toolchain:

```
rustup target add x86_64-pc-windows-gnu
rustup default stable-x86_64-pc-windows-gnu   # if you have mingw-w64 gcc installed
```

If you're on a machine where Git for Windows' `coreutils` shadows `link.exe`
in PATH (that was the actual problem on the original machine, not a missing
install — check with `Get-Command link.exe -All` first), just make sure the
real MSVC `link.exe` resolves ahead of it, or run from an "x64 Native Tools"
developer shell.

## Known risk spots to check first if `cargo build` fails

These are the lines I was least sure of without a compiler, in rough order of
likely trouble:

1. `src/main.rs`, `uEdge: ABE_BOTTOM` in `update_position()` — if this is a
   type mismatch, try `ABE_BOTTOM.0` or `ABE_BOTTOM.0 as u32` (depends on
   whether `windows-rs` typed `ABE_*`/`ABN_*` as plain `u32` consts or as a
   wrapped wrapper type for this crate version).
2. `CreateMutexW(None, BOOL(1), ...)` — parameter order/types for the
   `windows` crate's `CreateMutexW` binding; `BOOL` import is from
   `windows::Win32::Foundation`.
3. `HBRUSH(GetStockObject(BLACK_BRUSH).0)` — relies on `HGDIOBJ` and `HBRUSH`
   wrapping the same inner type so `.0` can be re-wrapped directly.
4. `hinstance.into()` (used twice) — converting `HMODULE` (from
   `GetModuleHandleW`) to `HINSTANCE` (needed by `WNDCLASSEXW`/
   `CreateWindowExW`). If there's no `From<HMODULE> for HINSTANCE`, cast via
   the raw pointer instead (`HINSTANCE(hinstance.0)`).
5. Cargo.toml pins `windows = "0.58"` — the registry has 0.62.x now. If you'd
   rather build against current, bump the version and re-check the above
   against that version's docs (https://docs.rs/windows, pick the version you
   land on in `Cargo.lock`), since Win32 binding shapes do shift between
   releases.

Fix pattern if something doesn't compile: the error message will name the
expected type — 9 times out of 10 it's "expected `u32`, found `ABE_EDGE`" or
the reverse, which tells you immediately whether to add or remove a `.0`.

## Context (why this exists)

This repo's current ShadowTaskbar is a stripped-down fork of a bigger WPF app
(AppSwitcherBar) and still drags in unused machinery (COM app-resolver, audio
device enumeration, DI/hosting, logging) for something that just needs to
reserve black screen space. This prototype is a from-scratch exploration of
whether a ~250-line Rust binary (sub-MB, no CLR startup, single-digit-MB idle
memory) can replace it. It was built under `/tmp` deliberately — nothing here
is meant to be committed to the repo as-is; if it works out, treat it as a
reference to port in, not a drop-in replacement.
