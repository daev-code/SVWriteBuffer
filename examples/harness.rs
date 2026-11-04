//! Test harness: run with `cargo run --release --example harness`.
//!
//! Writes a fake SavedVariables file token by token (like the game does) before and after
//! loading SVWriteBuffer.dll into this process, and checks the contents, a size query in
//! the middle of writing, and that other files are left alone.

use core::ffi::c_void;
use std::time::Instant;

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileA(n: *const u8, a: u32, s: u32, sec: *mut c_void, d: u32, f: u32, t: Handle) -> Handle;
    fn CreateFileW(n: *const u16, a: u32, s: u32, sec: *mut c_void, d: u32, f: u32, t: Handle) -> Handle;
    fn WriteFile(h: Handle, b: *const u8, n: u32, w: *mut u32, o: *mut c_void) -> i32;
    fn ReadFile(h: Handle, b: *mut u8, n: u32, r: *mut u32, o: *mut c_void) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn SetFilePointer(h: Handle, d: i32, hi: *mut i32, m: u32) -> u32;
    fn GetFileSize(h: Handle, hi: *mut u32) -> u32;
    fn SetEndOfFile(h: Handle) -> i32;
    fn FlushFileBuffers(h: Handle) -> i32;
    fn LoadLibraryW(n: *const u16) -> *mut c_void;
    fn GetModuleHandleW(n: *const u16) -> *mut c_void;
    fn GetProcAddress(m: *mut c_void, n: *const u8) -> usize;
}

const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;

fn tokens() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for i in 0..60_000u32 {
        for t in ["\t\t[\"", "item:", &i.to_string(), "\"] = ", "\"1793228400#2000#4499\"", ",\n"] {
            v.push(t.as_bytes().to_vec());
        }
    }
    v
}

unsafe fn write_tokens(path: &str, toks: &[Vec<u8>], probe_size_at: Option<usize>) -> (f64, Option<(u32, usize)>) {
    write_tokens_with(path, toks, probe_size_at, false)
}

unsafe fn write_tokens_with(path: &str, toks: &[Vec<u8>], probe_size_at: Option<usize>, wide: bool) -> (f64, Option<(u32, usize)>) {
    let h = if wide {
        let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
        CreateFileW(w.as_ptr(), GENERIC_WRITE, 0, core::ptr::null_mut(), CREATE_ALWAYS, 0x80, core::ptr::null_mut())
    } else {
        let c = std::ffi::CString::new(path).unwrap();
        CreateFileA(c.as_ptr() as *const u8, GENERIC_WRITE, 0, core::ptr::null_mut(), CREATE_ALWAYS, 0x80, core::ptr::null_mut())
    };
    assert!(h as isize != -1, "CreateFileA failed for {path}");
    let start = Instant::now();
    let mut probe = None;
    let mut so_far = 0usize;
    for (i, t) in toks.iter().enumerate() {
        if Some(i) == probe_size_at {
            probe = Some((GetFileSize(h, core::ptr::null_mut()), so_far));
        }
        let mut w = 0u32;
        assert!(WriteFile(h, t.as_ptr(), t.len() as u32, &mut w, core::ptr::null_mut()) != 0);
        assert_eq!(w as usize, t.len());
        so_far += t.len();
    }
    assert!(CloseHandle(h) != 0);
    (start.elapsed().as_secs_f64() * 1000.0, probe)
}

fn main() {
    // Keep every import the DLL looks for in this exe's import table.
    let keep = [ReadFile as *const () as usize, SetFilePointer as *const () as usize, SetEndOfFile as *const () as usize, FlushFileBuffers as *const () as usize];
    std::hint::black_box(keep);

    let dir = std::env::temp_dir().join("svwb-test").join("SavedVariables");
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_current_dir(dir.parent().unwrap()).unwrap();
    let sv = dir.join("TestAddon.lua").to_string_lossy().into_owned();
    let other = dir.parent().unwrap().join("other.txt").to_string_lossy().into_owned();

    let toks = tokens();
    let expected: Vec<u8> = toks.concat();
    println!("{} writes, {} bytes", toks.len(), expected.len());

    let (before, _) = unsafe { write_tokens(&sv, &toks, None) };
    assert_eq!(std::fs::read(&sv).unwrap(), expected);
    println!("without DLL: {before:.1} ms");

    // Like the game: look up kernel32 functions before the DLL loads, call them later.
    type CreateA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, Handle) -> Handle;
    type Write = unsafe extern "system" fn(Handle, *const u8, u32, *mut u32, *mut c_void) -> i32;
    type Close = unsafe extern "system" fn(Handle) -> i32;
    let (cached_create, cached_write, cached_close): (CreateA, Write, Close) = unsafe {
        let k32 = GetModuleHandleW("kernel32.dll\0".encode_utf16().collect::<Vec<u16>>().as_ptr());
        (
            core::mem::transmute(GetProcAddress(k32, b"CreateFileA\0".as_ptr())),
            core::mem::transmute(GetProcAddress(k32, b"WriteFile\0".as_ptr())),
            core::mem::transmute(GetProcAddress(k32, b"CloseHandle\0".as_ptr())),
        )
    };

    let dll = std::env::args().nth(1).expect("pass the DLL path");
    let wide: Vec<u16> = dll.encode_utf16().chain([0]).collect();
    assert!(!unsafe { LoadLibraryW(wide.as_ptr()) }.is_null(), "LoadLibraryW failed");

    let probe_at = toks.len() / 2;
    let (after, probe) = unsafe { write_tokens(&sv, &toks, Some(probe_at)) };
    assert_eq!(std::fs::read(&sv).unwrap(), expected, "SavedVariables content differs");
    let (size, so_far) = probe.unwrap();
    assert_eq!(size as usize, so_far, "GetFileSize mid-write must see everything written so far");
    println!("with DLL:    {after:.1} ms  (content OK, size probe OK)");

    let (_, _) = unsafe { write_tokens(&other, &toks[..1000], None) };
    assert_eq!(std::fs::read(&other).unwrap(), toks[..1000].concat());

    // CreateFileW path, as DLLs that save SavedVariables themselves may use.
    let sv_w = dir.join("WideAddon.lua");
    let (wms, _) = unsafe { write_tokens_with(&sv_w.to_string_lossy(), &toks, None, true) };
    println!("CreateFileW with DLL: {wms:.1} ms");
    assert_eq!(std::fs::read(&sv_w).unwrap(), expected, "CreateFileW content differs");

    let sv_c = dir.join("CachedAddon.lua").to_string_lossy().into_owned();
    unsafe {
        let c = std::ffi::CString::new(sv_c.clone()).unwrap();
        let h = cached_create(c.as_ptr() as *const u8, GENERIC_WRITE, 0, core::ptr::null_mut(), CREATE_ALWAYS, 0x80, core::ptr::null_mut());
        assert!(h as isize != -1);
        for t in &toks {
            let mut w = 0u32;
            assert!(cached_write(h, t.as_ptr(), t.len() as u32, &mut w, core::ptr::null_mut()) != 0);
        }
        assert!(cached_close(h) != 0);
    }
    assert_eq!(std::fs::read(&sv_c).unwrap(), expected, "cached-pointer content differs");

    let log_file = std::env::current_exe().unwrap().parent().unwrap().join("SVWriteBuffer.log");
    let log = std::fs::read_to_string(log_file).unwrap();
    print!("log:\n{log}");
    assert!(log.contains("active: patched kernel32"));
    assert!(log.lines().any(|l| l.contains(" 360000 writes ->") && l.contains("CachedAddon.lua")), "calls through pointers saved before loading must be buffered");
    assert!(log.lines().any(|l| l.contains("writes ->") && l.contains("TestAddon.lua")));
    assert!(log.lines().any(|l| l.contains(" 360000 writes ->") && l.contains("WideAddon.lua")), "CreateFileW file must be buffered");
    assert!(!log.lines().any(|l| l.contains("writes ->") && l.contains("other.txt")), "non-SavedVariables file must not be buffered");
    println!("ALL OK");
}
