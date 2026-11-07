//! SVWriteBuffer: makes the WoW 1.12 client write SavedVariables files in one go.
//!
//! SavedVariables are written with one `WriteFile` call per token (3-8 bytes on average),
//! so saving a few MB of addon data takes hundreds of thousands of system calls and
//! seconds of every reload and logout.
//!
//! On 32-bit Windows every kernel32 file function is a stub that jumps through kernel32's
//! own import table into kernelbase.dll. This DLL redirects those import entries, so it
//! sees every call made through kernel32, including calls through function pointers the
//! game saved at startup (patching the game's import table is not enough for that):
//!
//! - `CreateFileA` / `CreateFileW`: a file opened for writing whose path contains
//!   `SavedVariables` and ends in `.lua` is tracked.
//! - `WriteFile` on a tracked handle appends to a memory buffer and reports success.
//! - `CloseHandle` on a tracked handle writes the buffer in one call, then closes.
//! - `ReadFile`, `SetFilePointer(Ex)`, `GetFileSize(Ex)`, `SetEndOfFile` and
//!   `FlushFileBuffers` on a tracked handle write the buffer first, so the file is always
//!   consistent when anyone looks at it.
//!
//! The data reaches the disk before `CloseHandle` returns, at the same point as before,
//! so nothing changes for crashes or for the next load reading the file. Untracked
//! handles go straight to kernelbase. Only entries that still point at kernelbase are
//! patched: if another DLL already redirected one, it is left alone, and if `CreateFile`,
//! `WriteFile` and `CloseHandle` can't all be patched, nothing is.
//!
//! The DLL's own log writes go through the hooked functions too; a per-thread flag makes
//! the hooks pass those straight through.
//!
//! Each game start replaces `SVWriteBuffer.log` next to the game executable: what was
//! patched, and one line per buffered file (size, writes made, writes that reached the
//! disk, time). Players attach it to bug reports, so it never contains paths: only the
//! file name and whether it is per account or per character. It stops at `MAX_LOG`.

#![cfg(all(windows, target_arch = "x86"))]
#![allow(non_snake_case)] // the crate is named after the DLL file

use core::ffi::c_void;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

type Handle = *mut c_void;
type Bool = i32;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;
const INVALID_HANDLE_VALUE: isize = -1;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const PAGE_READWRITE: u32 = 0x04;
/// Write out early if one file's buffer grows past this.
const MAX_BUFFER: usize = 64 * 1024 * 1024;
const LOG_NAME: &str = "SVWriteBuffer.log";
/// The log stops growing past this many bytes in one session.
const MAX_LOG: usize = 1024 * 1024;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> usize;
    fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> Bool;
    fn DisableThreadLibraryCalls(module: *mut c_void) -> Bool;
    fn QueryPerformanceCounter(count: *mut i64) -> Bool;
    fn QueryPerformanceFrequency(freq: *mut i64) -> Bool;
}

type CreateFileAFn = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, Handle) -> Handle;
type CreateFileWFn = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, Handle) -> Handle;
type WriteFileFn = unsafe extern "system" fn(Handle, *const u8, u32, *mut u32, *mut c_void) -> Bool;
type ReadFileFn = unsafe extern "system" fn(Handle, *mut u8, u32, *mut u32, *mut c_void) -> Bool;
type CloseHandleFn = unsafe extern "system" fn(Handle) -> Bool;
type SetFilePointerFn = unsafe extern "system" fn(Handle, i32, *mut i32, u32) -> u32;
type SetFilePointerExFn = unsafe extern "system" fn(Handle, i64, *mut i64, u32) -> Bool;
type GetFileSizeFn = unsafe extern "system" fn(Handle, *mut u32) -> u32;
type GetFileSizeExFn = unsafe extern "system" fn(Handle, *mut i64) -> Bool;
type HandleBoolFn = unsafe extern "system" fn(Handle) -> Bool;

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Open,
    Write,
    Close,
    Observe,
}

struct Func {
    name: &'static str,
    role: Role,
    /// The kernelbase export: what kernel32's stub would have jumped to.
    orig: AtomicUsize,
}

macro_rules! func {
    ($name:literal, $role:expr) => {
        Func { name: $name, role: $role, orig: AtomicUsize::new(0) }
    };
}

static FUNCS: [Func; 11] = [
    func!("CreateFileA", Role::Open),
    func!("CreateFileW", Role::Open),
    func!("WriteFile", Role::Write),
    func!("CloseHandle", Role::Close),
    func!("ReadFile", Role::Observe),
    func!("SetFilePointer", Role::Observe),
    func!("SetFilePointerEx", Role::Observe),
    func!("GetFileSize", Role::Observe),
    func!("GetFileSizeEx", Role::Observe),
    func!("SetEndOfFile", Role::Observe),
    func!("FlushFileBuffers", Role::Observe),
];

fn hook_addr(i: usize) -> usize {
    let f: [usize; 11] = [
        hk_create_file_a as *const () as usize,
        hk_create_file_w as *const () as usize,
        hk_write_file as *const () as usize,
        hk_close_handle as *const () as usize,
        hk_read_file as *const () as usize,
        hk_set_file_pointer as *const () as usize,
        hk_set_file_pointer_ex as *const () as usize,
        hk_get_file_size as *const () as usize,
        hk_get_file_size_ex as *const () as usize,
        hk_set_end_of_file as *const () as usize,
        hk_flush_file_buffers as *const () as usize,
    ];
    f[i]
}

const I_CREATE_A: usize = 0;
const I_CREATE_W: usize = 1;
const I_WRITE: usize = 2;
const I_CLOSE: usize = 3;
const I_READ: usize = 4;
const I_SETPTR: usize = 5;
const I_SETPTR_EX: usize = 6;
const I_GETSIZE: usize = 7;
const I_GETSIZE_EX: usize = 8;
const I_SETEOF: usize = 9;
const I_FLUSH: usize = 10;

/// Number of tracked handles, so untracked calls skip the lock.
static TRACKED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Set while this DLL does its own file work (logging), so the hooks pass it through.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

fn busy() -> bool {
    BUSY.with(|b| b.get())
}

struct Tracked {
    name: String,
    buf: Vec<u8>,
    calls: u64,
    bytes: u64,
    flush_ticks: i64,
    flushes: u32,
}

static FILES: Mutex<BTreeMap<usize, Tracked>> = Mutex::new(BTreeMap::new());

fn files() -> std::sync::MutexGuard<'static, BTreeMap<usize, Tracked>> {
    FILES.lock().unwrap_or_else(|e| e.into_inner())
}

fn ticks() -> i64 {
    let mut t = 0i64;
    unsafe { QueryPerformanceCounter(&mut t) };
    t
}

fn ms(ticks: i64) -> f64 {
    let mut f = 0i64;
    unsafe { QueryPerformanceFrequency(&mut f) };
    if f == 0 { 0.0 } else { ticks as f64 * 1000.0 / f as f64 }
}

fn log_path() -> std::path::PathBuf {
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(LOG_NAME)))
            .unwrap_or_else(|| std::path::PathBuf::from(LOG_NAME))
    })
    .clone()
}

fn log(line: &str) {
    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    let n = line.len() + 1;
    let before = LOGGED.fetch_add(n, Ordering::Relaxed);
    if before >= MAX_LOG {
        return;
    }
    let full = before + n > MAX_LOG;
    let was = BUSY.with(|b| b.replace(true));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = if full {
            writeln!(f, "log full ({} KB), later lines dropped until the game restarts", MAX_LOG / 1024)
        } else {
            writeln!(f, "{line}")
        };
    }
    BUSY.with(|b| b.set(was));
}

/// What the log shows for a SavedVariables path: the file name and whether it is per
/// account or per character, never the folders (they hold account and character names).
fn display_name(path: &str) -> String {
    let parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let Some(last) = parts.len().checked_sub(1) else { return String::new() };
    // The folder whose contents are the file: `...\SavedVariables\<file>`, or the file
    // itself for the client's own `...\SavedVariables.lua`.
    let dir = parts.iter().rposition(|p| p.eq_ignore_ascii_case("savedvariables")).unwrap_or(last);
    let file = if dir < last { parts[dir + 1..].join("\\") } else { parts[last].to_string() };
    // WTF\Account\<account>\SavedVariables or WTF\Account\<account>\<realm>\<character>\SavedVariables
    let depth = parts[..dir].iter().rposition(|p| p.eq_ignore_ascii_case("account")).map(|a| dir - a - 1);
    match depth {
        Some(1) => format!("{file} (account)"),
        Some(3) => format!("{file} (character)"),
        _ => file,
    }
}

unsafe fn orig<T: Copy>(i: usize) -> T {
    let p = FUNCS[i].orig.load(Ordering::Relaxed);
    core::mem::transmute_copy::<usize, T>(&p)
}

/// Writes the whole buffer with the original WriteFile. Returns false on failure.
unsafe fn write_all(h: Handle, data: &[u8]) -> bool {
    let write: WriteFileFn = orig(I_WRITE);
    let mut off = 0usize;
    while off < data.len() {
        let chunk = (data.len() - off).min(16 * 1024 * 1024) as u32;
        let mut done = 0u32;
        if write(h, data.as_ptr().add(off), chunk, &mut done, core::ptr::null_mut()) == 0 || done == 0 {
            return false;
        }
        off += done as usize;
    }
    true
}

unsafe fn flush_entry(h: Handle, t: &mut Tracked) -> bool {
    if t.buf.is_empty() {
        return true;
    }
    let start = ticks();
    let ok = write_all(h, &t.buf);
    t.flush_ticks += ticks() - start;
    t.flushes += 1;
    t.buf.clear();
    if !ok {
        log(&format!("write failed: {}", t.name));
    }
    ok
}

/// Writes out a tracked handle's buffer (if it is tracked) before another operation.
unsafe fn flush_if_tracked(h: Handle) {
    if TRACKED.load(Ordering::Relaxed) == 0 || busy() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut map = files();
        if let Some(t) = map.get_mut(&(h as usize)) {
            flush_entry(h, t);
        }
    }));
}

fn is_saved_variables(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("savedvariables") && lower.ends_with(".lua")
}

/// Starts tracking a handle that was just opened, if it is a SavedVariables file.
unsafe fn maybe_track(h: Handle, path: Option<String>, access: u32, flags: u32) {
    if busy()
        || h as isize == INVALID_HANDLE_VALUE
        || h.is_null()
        || access & GENERIC_WRITE == 0
        || flags & (FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING) != 0
    {
        return;
    }
    let Some(path) = path else { return };
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if is_saved_variables(&path) {
            let mut map = files();
            let entry = Tracked { name: display_name(&path), buf: Vec::with_capacity(256 * 1024), calls: 0, bytes: 0, flush_ticks: 0, flushes: 0 };
            if map.insert(h as usize, entry).is_none() {
                TRACKED.fetch_add(1, Ordering::Relaxed);
            }
        }
    }));
}

// --- Hooks ------------------------------------------------------------------------------

unsafe extern "system" fn hk_create_file_a(
    name: *const u8, access: u32, share: u32, sec: *mut c_void, disp: u32, flags: u32, template: Handle,
) -> Handle {
    let create: CreateFileAFn = orig(I_CREATE_A);
    let h = create(name, access, share, sec, disp, flags, template);
    if access & GENERIC_WRITE != 0 && !name.is_null() {
        let path = catch_unwind(|| std::ffi::CStr::from_ptr(name as *const i8).to_string_lossy().into_owned()).ok();
        maybe_track(h, path, access, flags);
    }
    h
}

unsafe extern "system" fn hk_create_file_w(
    name: *const u16, access: u32, share: u32, sec: *mut c_void, disp: u32, flags: u32, template: Handle,
) -> Handle {
    let create: CreateFileWFn = orig(I_CREATE_W);
    let h = create(name, access, share, sec, disp, flags, template);
    if access & GENERIC_WRITE != 0 && !name.is_null() {
        let path = catch_unwind(|| {
            let mut n = 0usize;
            while *name.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(core::slice::from_raw_parts(name, n))
        })
        .ok();
        maybe_track(h, path, access, flags);
    }
    h
}

unsafe extern "system" fn hk_write_file(
    h: Handle, data: *const u8, len: u32, written: *mut u32, overlapped: *mut c_void,
) -> Bool {
    if TRACKED.load(Ordering::Relaxed) > 0 && !busy() && overlapped.is_null() && (!data.is_null() || len == 0) {
        let handled = catch_unwind(AssertUnwindSafe(|| {
            let mut map = files();
            let Some(t) = map.get_mut(&(h as usize)) else { return false };
            if t.buf.len() + len as usize > MAX_BUFFER && !flush_entry(h, t) {
                return false;
            }
            if len > 0 {
                t.buf.extend_from_slice(core::slice::from_raw_parts(data, len as usize));
            }
            t.calls += 1;
            t.bytes += len as u64;
            true
        }));
        if matches!(handled, Ok(true)) {
            if !written.is_null() {
                *written = len;
            }
            return 1;
        }
    }
    let write: WriteFileFn = orig(I_WRITE);
    write(h, data, len, written, overlapped)
}

unsafe extern "system" fn hk_close_handle(h: Handle) -> Bool {
    if TRACKED.load(Ordering::Relaxed) > 0 && !busy() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let entry = files().remove(&(h as usize));
            if let Some(mut t) = entry {
                TRACKED.fetch_sub(1, Ordering::Relaxed);
                flush_entry(h, &mut t);
                log(&format!(
                    "{:>9} bytes  {:>7} writes -> {}  {:>7.1} ms  {}",
                    t.bytes, t.calls, t.flushes, ms(t.flush_ticks), t.name
                ));
            }
        }));
    }
    let close: CloseHandleFn = orig(I_CLOSE);
    close(h)
}

unsafe extern "system" fn hk_read_file(h: Handle, data: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool {
    flush_if_tracked(h);
    let f: ReadFileFn = orig(I_READ);
    f(h, data, len, read, ov)
}

unsafe extern "system" fn hk_set_file_pointer(h: Handle, dist: i32, high: *mut i32, method: u32) -> u32 {
    flush_if_tracked(h);
    let f: SetFilePointerFn = orig(I_SETPTR);
    f(h, dist, high, method)
}

unsafe extern "system" fn hk_set_file_pointer_ex(h: Handle, dist: i64, newpos: *mut i64, method: u32) -> Bool {
    flush_if_tracked(h);
    let f: SetFilePointerExFn = orig(I_SETPTR_EX);
    f(h, dist, newpos, method)
}

unsafe extern "system" fn hk_get_file_size(h: Handle, high: *mut u32) -> u32 {
    flush_if_tracked(h);
    let f: GetFileSizeFn = orig(I_GETSIZE);
    f(h, high)
}

unsafe extern "system" fn hk_get_file_size_ex(h: Handle, size: *mut i64) -> Bool {
    flush_if_tracked(h);
    let f: GetFileSizeExFn = orig(I_GETSIZE_EX);
    f(h, size)
}

unsafe extern "system" fn hk_set_end_of_file(h: Handle) -> Bool {
    flush_if_tracked(h);
    let f: HandleBoolFn = orig(I_SETEOF);
    f(h)
}

unsafe extern "system" fn hk_flush_file_buffers(h: Handle) -> Bool {
    flush_if_tracked(h);
    let f: HandleBoolFn = orig(I_FLUSH);
    f(h)
}

// --- Import table patching --------------------------------------------------------------

unsafe fn rd32(p: *const u8) -> u32 {
    core::ptr::read_unaligned(p as *const u32)
}

unsafe fn cstr<'a>(p: *const u8) -> &'a [u8] {
    std::ffi::CStr::from_ptr(p as *const i8).to_bytes()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Looks up the kernelbase addresses of the hooked functions; these become the originals.
unsafe fn resolve_originals() -> Result<(), String> {
    let kb = GetModuleHandleW(wide("kernelbase.dll").as_ptr());
    if kb.is_null() {
        return Err("kernelbase not loaded".into());
    }
    for f in FUNCS.iter() {
        let name = std::ffi::CString::new(f.name).unwrap();
        let a = GetProcAddress(kb, name.as_ptr() as *const u8);
        if a == 0 {
            return Err(format!("kernelbase has no {}", f.name));
        }
        f.orig.store(a, Ordering::Relaxed);
    }
    Ok(())
}

/// kernel32's import slots for the hooked functions: (function index, slot address).
unsafe fn kernel32_slots(base: *mut u8) -> Result<Vec<(usize, *mut usize)>, String> {
    let nt = base.add(rd32(base.add(0x3C)) as usize);
    if rd32(nt) != 0x0000_4550 || core::ptr::read_unaligned(nt.add(24) as *const u16) != 0x10B {
        return Err("kernel32 is not a PE32 image".into());
    }
    let import_rva = rd32(nt.add(24 + 96 + 8)) as usize;
    if import_rva == 0 {
        return Err("kernel32 has no import table".into());
    }
    let mut found = Vec::new();
    let mut desc = base.add(import_rva);
    loop {
        let oft = rd32(desc) as usize;
        let name_rva = rd32(desc.add(12)) as usize;
        let ft = rd32(desc.add(16)) as usize;
        if name_rva == 0 && ft == 0 {
            break;
        }
        if oft != 0 {
            let mut i = 0usize;
            loop {
                let thunk = rd32(base.add(oft + i * 4));
                if thunk == 0 {
                    break;
                }
                if thunk & 0x8000_0000 == 0 {
                    let fname = cstr(base.add(thunk as usize + 2));
                    if let Some(k) = FUNCS.iter().position(|f| f.name.as_bytes() == fname) {
                        found.push((k, base.add(ft + i * 4) as *mut usize));
                    }
                }
                i += 1;
            }
        }
        desc = desc.add(20);
    }
    Ok(found)
}

/// Patches kernel32's import entries. Returns the names patched and the names skipped.
unsafe fn patch_kernel32() -> Result<(Vec<&'static str>, Vec<&'static str>), String> {
    let base = GetModuleHandleW(wide("kernel32.dll").as_ptr());
    if base.is_null() {
        return Err("kernel32 not loaded".into());
    }
    let slots = kernel32_slots(base)?;
    let mut ok = Vec::new();
    let mut skipped = Vec::new();
    for (k, slot) in &slots {
        let v = core::ptr::read_volatile(*slot);
        if v == FUNCS[*k].orig.load(Ordering::Relaxed) || v == hook_addr(*k) {
            ok.push((*k, *slot));
        } else {
            skipped.push(FUNCS[*k].name);
        }
    }
    let has = |role: Role| ok.iter().any(|(k, _)| FUNCS[*k].role == role);
    if !has(Role::Open) || !has(Role::Write) || !has(Role::Close) {
        return Err(format!(
            "CreateFile, WriteFile and CloseHandle can't all be patched (found {}, already redirected by something else: {:?})",
            slots.len(), skipped
        ));
    }
    let mut names = Vec::new();
    for (k, slot) in &ok {
        let mut old = 0u32;
        if VirtualProtect(*slot as *mut c_void, 4, PAGE_READWRITE, &mut old) == 0 {
            return Err(format!("VirtualProtect failed after {:?}", names));
        }
        core::ptr::write_volatile(*slot, hook_addr(*k));
        let mut tmp = 0u32;
        VirtualProtect(*slot as *mut c_void, 4, old, &mut tmp);
        names.push(FUNCS[*k].name);
    }
    Ok((names, skipped))
}

/// On unload or exit: write out anything still buffered.
unsafe fn flush_all() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut map = files();
        for (h, t) in map.iter_mut() {
            flush_entry(*h as Handle, t);
        }
    }));
}

#[no_mangle]
pub extern "system" fn DllMain(module: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    unsafe {
        if reason == DLL_PROCESS_ATTACH {
            DisableThreadLibraryCalls(module);
            let _ = std::fs::remove_file(log_path());
            log(&format!("SVWriteBuffer {}", env!("CARGO_PKG_VERSION")));
            let result = catch_unwind(|| {
                resolve_originals()?;
                patch_kernel32()
            });
            match result {
                Ok(Ok((names, skipped))) => {
                    log(&format!("active: patched kernel32 {}", names.join(" ")));
                    if !skipped.is_empty() {
                        log(&format!("left alone (already redirected by something else): {}", skipped.join(" ")));
                    }
                }
                Ok(Err(e)) => log(&format!("not active: {e}")),
                Err(_) => log("not active: panic while patching"),
            }
        } else if reason == DLL_PROCESS_DETACH {
            flush_all();
        }
    }
    1
}
