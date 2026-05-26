#![allow(non_snake_case)]

/// Win7 compatibility shims for Win8+ APIs statically imported by Rust std and Slint.
///
/// Strategy:
///  1. Define Win7-compatible stub implementations (compat_* functions).
///  2. Define `__imp_*` static pointers pointing to those stubs.
///     This overrides the import address table (IAT) entries that the import
///     libraries inside libstd.rlib / windows_core.rlib would otherwise provide,
///     so calls through the IAT go to our stubs, not the DLL.
///  3. `/FORCE:MULTIPLE` in build.rs lets the linker accept both our definition
///     and the one from the embedded import lib — ours (first in link order) wins.
///  4. `/DELAYLOAD:DLL` in build.rs moves each DLL from the regular import table
///     to the delay-load table, so Windows does NOT demand-load them at startup.
///     Because our `__imp_*` statics never trigger the delay-load thunk, the DLL
///     is never actually loaded → Win7 works fine.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};
use windows_sys::Win32::Foundation::{BOOL, TRUE, FALSE};

// ── kernel32.dll (Win8+) ──────────────────────────────────────────────────────
// GetSystemTimePreciseAsFileTime — high-precision clock added in Win8.
// Fallback: GetSystemTimeAsFileTime (lower precision, available on Win7).

unsafe extern "system" fn compat_GetSystemTimePreciseAsFileTime(lp_system_time_as_file_time: *mut u64) {
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    GetSystemTimeAsFileTime(lp_system_time_as_file_time as *mut _);
}

#[used] #[no_mangle]
pub static __imp_GetSystemTimePreciseAsFileTime:
    unsafe extern "system" fn(*mut u64)
    = compat_GetSystemTimePreciseAsFileTime;

// ── api-ms-win-crt-runtime-l1-1-0.dll  (VS2019+ UCRT functions) ──────────────
// These CRT startup functions were added in VS2019 (UCRT 14.28+).
// Win7 with VS2015 redistributable has UCRT 14.0 which lacks them.
// Stubs are safe for a GUI app that ignores argv/environment.

// Configures how argv is parsed. For GUI apps: no-op.
unsafe extern "C" fn compat_configure_narrow_argv(_: *const c_void) {}

// Initialises the narrow (ANSI) environment table. Return 0 = success.
unsafe extern "C" fn compat_initialize_narrow_environment() -> i32 { 0 }

// Returns a pointer to the initial narrow environment. Return NULL = empty.
unsafe extern "C" fn compat_get_initial_narrow_environment() -> *mut *mut u8 {
    core::ptr::null_mut()
}

// Registers a thread-local destructor callback. No-op = callbacks are skipped.
unsafe extern "C" fn compat_register_thread_local_exe_atexit_callback(_: *const c_void) -> i32 { 0 }

// Wide-char equivalents (same pattern, called when /UNICODE is set).
unsafe extern "C" fn compat_configure_wide_argv(_: *const c_void) {}
unsafe extern "C" fn compat_initialize_wide_environment() -> i32 { 0 }
unsafe extern "C" fn compat_get_initial_wide_environment() -> *mut *mut u16 {
    core::ptr::null_mut()
}

#[used] #[no_mangle] pub static __imp__configure_narrow_argv:
    unsafe extern "C" fn(*const c_void) = compat_configure_narrow_argv;
#[used] #[no_mangle] pub static __imp__initialize_narrow_environment:
    unsafe extern "C" fn() -> i32 = compat_initialize_narrow_environment;
#[used] #[no_mangle] pub static __imp__get_initial_narrow_environment:
    unsafe extern "C" fn() -> *mut *mut u8 = compat_get_initial_narrow_environment;
#[used] #[no_mangle] pub static __imp__register_thread_local_exe_atexit_callback:
    unsafe extern "C" fn(*const c_void) -> i32 = compat_register_thread_local_exe_atexit_callback;
#[used] #[no_mangle] pub static __imp__configure_wide_argv:
    unsafe extern "C" fn(*const c_void) = compat_configure_wide_argv;
#[used] #[no_mangle] pub static __imp__initialize_wide_environment:
    unsafe extern "C" fn() -> i32 = compat_initialize_wide_environment;
#[used] #[no_mangle] pub static __imp__get_initial_wide_environment:
    unsafe extern "C" fn() -> *mut *mut u16 = compat_get_initial_wide_environment;

/// Call once at program start (before slint::platform::set_platform) to
/// prevent LTO/LTCG from dead-stripping the __imp_* override statics.
pub fn init() {
    use core::hint::black_box;
    black_box(&__imp_WaitOnAddress          as *const _);
    black_box(&__imp_WakeByAddressAll       as *const _);
    black_box(&__imp_WakeByAddressSingle    as *const _);
    black_box(&__imp_ProcessPrng            as *const _);
    black_box(&__imp_RoOriginateErrorW      as *const _);
    black_box(&__imp_CoTaskMemFree          as *const _);
    black_box(&__imp_CoCreateFreeThreadedMarshaler as *const _);
    black_box(&__imp_GetSystemTimePreciseAsFileTime as *const _);
    black_box(&__imp__configure_narrow_argv as *const _);
    black_box(&__imp__initialize_narrow_environment as *const _);
    black_box(&__imp__get_initial_narrow_environment as *const _);
    black_box(&__imp__register_thread_local_exe_atexit_callback as *const _);
    black_box(&__imp__configure_wide_argv as *const _);
    black_box(&__imp__initialize_wide_environment as *const _);
    black_box(&__imp__get_initial_wide_environment as *const _);
    black_box(&DELAY_FAILURE_HOOK           as *const _);
}

// ─────────────────────────────────────────────────────────────────────────────
// MSVC delay-load failure hook  (__pfnDliFailureHook2)
//
// When a delay-loaded DLL is not found (e.g. bcryptprimitives.dll on Win7),
// the delay-load helper calls this hook instead of raising an exception.
//
// Flow for each missing DLL:
//   1. dliFailLoadLib (=3)  → we return any non-null fake HMODULE
//   2. GetProcAddress(fake_hmod, name) fails
//   3. dliFailGetProc (=4)  → we return the address of our shim function
//   4. The thunk writes that address into the delay-load IAT slot
//   5. All subsequent calls go directly to our shim (no more thunk)
// ─────────────────────────────────────────────────────────────────────────────

// Mirror of MSVC's DelayLoadProc (from <delayimp.h>).
// On i686 (32-bit), all pointers and BOOL are 4 bytes.
#[repr(C)]
struct DliProc {
    f_import_by_name: u32,   // 0 = by ordinal, else by name
    ordinal_or_name:  u32,   // dwOrdinal OR pointer to name string (i686: 32-bit addr)
}

// Mirror of MSVC's DelayLoadInfo (from <delayimp.h>).
#[repr(C)]
struct DliInfo {
    cb:           u32,
    pidd:         *const c_void,
    ppfn:         *mut *mut c_void,
    sz_dll:       *const u8,
    dlp:          DliProc,
    hmod_cur:     u32,           // HMODULE on i686
    pfn_cur:      *mut c_void,
    dw_last_error: u32,
}

// Byte-exact case-sensitive comparison of a C string with a Rust byte literal.
unsafe fn cstr_eq(ptr: *const u8, expected: &[u8]) -> bool {
    for (i, &e) in expected.iter().enumerate() {
        if *ptr.add(i) != e { return false; }
    }
    *ptr.add(expected.len()) == 0
}

// Case-insensitive suffix check (ASCII only) — DLL names are short ASCII.
unsafe fn dll_name_is(ptr: *const u8, expected_lower: &[u8]) -> bool {
    let mut len = 0usize;
    while *ptr.add(len) != 0 { len += 1; }
    if len < expected_lower.len() { return false; }
    let offset = len - expected_lower.len();
    for (i, &e) in expected_lower.iter().enumerate() {
        let b = *ptr.add(offset + i);
        let b = if b >= b'A' && b <= b'Z' { b + 32 } else { b };
        if b != e { return false; }
    }
    true
}

unsafe fn shim_for_proc(info: &DliInfo) -> *mut c_void {
    let dll = info.sz_dll;
    let by_name = info.dlp.f_import_by_name != 0;
    let raw     = info.dlp.ordinal_or_name;

    // Helper: compare proc name when importing by name.
    // raw is a 32-bit VA pointing to the name string (on i686).
    macro_rules! name_is {
        ($lit:expr) => {
            by_name && cstr_eq(raw as *const u8, $lit)
        };
    }

    // bcryptprimitives.dll — ProcessPrng (ordinal 1 or by name)
    if dll_name_is(dll, b"bcryptprimitives.dll") {
        if (!by_name && raw == 1) || name_is!(b"ProcessPrng") {
            return compat_ProcessPrng as *mut c_void;
        }
    }

    // api-ms-win-core-synch-l1-2-0.dll — WaitOnAddress / WakeByAddress*
    if dll_name_is(dll, b"api-ms-win-core-synch-l1-2-0.dll") {
        if name_is!(b"WaitOnAddress")      { return compat_WaitOnAddress      as *mut c_void; }
        if name_is!(b"WakeByAddressAll")   { return compat_WakeByAddressAll   as *mut c_void; }
        if name_is!(b"WakeByAddressSingle"){ return compat_WakeByAddressSingle as *mut c_void; }
    }

    // api-ms-win-core-winrt-error-l1-1-0.dll — RoOriginateErrorW
    if dll_name_is(dll, b"api-ms-win-core-winrt-error-l1-1-0.dll") {
        if name_is!(b"RoOriginateErrorW") { return compat_RoOriginateErrorW as *mut c_void; }
    }

    // combase.dll — CoTaskMemFree / CoCreateFreeThreadedMarshaler
    if dll_name_is(dll, b"combase.dll") {
        if name_is!(b"CoTaskMemFree")                { return compat_CoTaskMemFree                as *mut c_void; }
        if name_is!(b"CoCreateFreeThreadedMarshaler") { return compat_CoCreateFreeThreadedMarshaler as *mut c_void; }
    }

    // api-ms-win-crt-runtime-l1-1-0.dll — CRT startup stubs
    if dll_name_is(dll, b"api-ms-win-crt-runtime-l1-1-0.dll") {
        if name_is!(b"_configure_narrow_argv")                      { return compat_configure_narrow_argv                      as *mut c_void; }
        if name_is!(b"_initialize_narrow_environment")               { return compat_initialize_narrow_environment               as *mut c_void; }
        if name_is!(b"_get_initial_narrow_environment")              { return compat_get_initial_narrow_environment              as *mut c_void; }
        if name_is!(b"_register_thread_local_exe_atexit_callback")   { return compat_register_thread_local_exe_atexit_callback   as *mut c_void; }
        if name_is!(b"_configure_wide_argv")                        { return compat_configure_wide_argv                        as *mut c_void; }
        if name_is!(b"_initialize_wide_environment")                 { return compat_initialize_wide_environment                 as *mut c_void; }
        if name_is!(b"_get_initial_wide_environment")                { return compat_get_initial_wide_environment                as *mut c_void; }
    }

    core::ptr::null_mut()
}

unsafe extern "system" fn dli_failure_hook(
    notify: u32,
    info: *const DliInfo,
) -> *mut c_void {
    const DLI_FAIL_LOAD_LIB: u32 = 3;
    const DLI_FAIL_GET_PROC: u32 = 4;

    match notify {
        DLI_FAIL_LOAD_LIB => {
            // Non-null fake HMODULE → tells the thunk "DLL was loaded".
            // The subsequent GetProcAddress(fake_hmod, ...) will fail, triggering
            // dliFailGetProc where we return the real shim address.
            0xDEAD_0001_u32 as usize as *mut c_void
        }
        DLI_FAIL_GET_PROC => shim_for_proc(&*info),
        _ => core::ptr::null_mut(),
    }
}

type PfnDliHook = Option<unsafe extern "system" fn(u32, *const DliInfo) -> *mut c_void>;

// Override ___pfnDliFailureHook2 from delayimp.lib (which defaults to NULL).
// Symbol uses 3 underscores: cdecl prepends one '_' to the C name '__pfnDliFailureHook2'.
// Our Rust object file is linked before delayimp.lib, so this definition wins
// over delayimp.lib's NULL version when /FORCE:MULTIPLE is in effect.
#[used]
#[no_mangle]
#[export_name = "___pfnDliFailureHook2"]
pub static DELAY_FAILURE_HOOK: PfnDliHook = Some(dli_failure_hook);

// ─────────────────────────────────────────────────────────────────────────────
// api-ms-win-core-synch-l1-2-0.dll  (Win8+)
// WaitOnAddress / WakeByAddressAll / WakeByAddressSingle
// Used by Rust std's mutex/condvar and thread::park on Windows.
// ─────────────────────────────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "system" fn compat_WaitOnAddress(
    address: *const c_void,
    compare_address: *const c_void,
    address_size: usize,
    dw_milliseconds: u32,
) -> BOOL {
    use windows_sys::Win32::System::Threading::Sleep;
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::Foundation::{SetLastError, ERROR_TIMEOUT};

    let deadline: u64 = if dw_milliseconds == u32::MAX {
        u64::MAX
    } else {
        (GetTickCount() as u64).wrapping_add(dw_milliseconds as u64)
    };

    loop {
        let changed = match address_size {
            1 => *(address as *const u8)  != *(compare_address as *const u8),
            2 => *(address as *const u16) != *(compare_address as *const u16),
            4 => *(address as *const u32) != *(compare_address as *const u32),
            8 => *(address as *const u64) != *(compare_address as *const u64),
            _ => return TRUE,
        };
        if changed { return TRUE; }
        if (GetTickCount() as u64) >= deadline {
            SetLastError(ERROR_TIMEOUT);
            return FALSE;
        }
        Sleep(1);
    }
}

#[no_mangle] pub unsafe extern "system" fn compat_WakeByAddressAll(_address: *const c_void) {}
#[no_mangle] pub unsafe extern "system" fn compat_WakeByAddressSingle(_address: *const c_void) {}

#[used] #[no_mangle]
pub static __imp_WaitOnAddress:
    unsafe extern "system" fn(*const c_void, *const c_void, usize, u32) -> BOOL
    = compat_WaitOnAddress;

#[used] #[no_mangle]
pub static __imp_WakeByAddressAll:
    unsafe extern "system" fn(*const c_void)
    = compat_WakeByAddressAll;

#[used] #[no_mangle]
pub static __imp_WakeByAddressSingle:
    unsafe extern "system" fn(*const c_void)
    = compat_WakeByAddressSingle;

// ─────────────────────────────────────────────────────────────────────────────
// bcryptprimitives.dll  (ProcessPrng added in Win8.1)
// Used by Rust std for HashMap seed / thread-local random state.
// Fallback: BCryptGenRandom with BCRYPT_USE_SYSTEM_PREFERRED_RNG (Vista+).
// ─────────────────────────────────────────────────────────────────────────────

unsafe extern "system" fn compat_ProcessPrng(pb: *mut u8, cb: usize) -> BOOL {
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let status = BCryptGenRandom(core::ptr::null_mut(), pb, cb as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    if status >= 0 { TRUE } else { FALSE }
}

#[used] #[no_mangle]
pub static __imp_ProcessPrng:
    unsafe extern "system" fn(*mut u8, usize) -> BOOL
    = compat_ProcessPrng;

// ─────────────────────────────────────────────────────────────────────────────
// api-ms-win-core-winrt-error-l1-1-0.dll  (Win8+)
// RoOriginateErrorW – WinRT-style COM error reporting used by Slint.
// No-op on Win7: WinRT doesn't exist, returning FALSE is safe.
// ─────────────────────────────────────────────────────────────────────────────

unsafe extern "system" fn compat_RoOriginateErrorW(
    _error: i32,
    _cch_msg: u32,
    _msg: *const u16,
) -> BOOL {
    FALSE
}

#[used] #[no_mangle]
pub static __imp_RoOriginateErrorW:
    unsafe extern "system" fn(i32, u32, *const u16) -> BOOL
    = compat_RoOriginateErrorW;

// ─────────────────────────────────────────────────────────────────────────────
// combase.dll  (Win8+)
// CoTaskMemFree / CoCreateFreeThreadedMarshaler — used by Slint's DirectWrite
// COM objects.  On Win7, these live in ole32.dll.  We load from ole32 at
// runtime so the same binary runs on Win7 (ole32) and Win8+ (combase or ole32).
// ─────────────────────────────────────────────────────────────────────────────

// One-time initialisation without std Mutex (which itself uses WaitOnAddress).
static OLE32_INIT: AtomicU8 = AtomicU8::new(0); // 0=uninit 1=busy 2=done
static mut FN_CO_TASK_MEM_FREE: usize = 0;
static mut FN_CO_CREATE_FTM: usize = 0;

unsafe fn load_ole32_fns() {
    loop {
        match OLE32_INIT.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                const OLE32_W: &[u16] = &[
                    b'o' as u16, b'l' as u16, b'e' as u16,
                    b'3' as u16, b'2' as u16, b'.' as u16,
                    b'd' as u16, b'l' as u16, b'l' as u16, 0,
                ];
                use windows_sys::Win32::System::LibraryLoader::{LoadLibraryW, GetProcAddress};
                let h = LoadLibraryW(OLE32_W.as_ptr());
                if h != 0 {
                    FN_CO_TASK_MEM_FREE = GetProcAddress(h, b"CoTaskMemFree\0".as_ptr())
                        .map(|f| f as usize).unwrap_or(0);
                    FN_CO_CREATE_FTM = GetProcAddress(h, b"CoCreateFreeThreadedMarshaler\0".as_ptr())
                        .map(|f| f as usize).unwrap_or(0);
                }
                OLE32_INIT.store(2, Ordering::Release);
                return;
            }
            Err(1) => { core::hint::spin_loop(); } // busy-wait
            Err(2) | Err(_) => return,             // done
        }
    }
}

unsafe extern "system" fn compat_CoTaskMemFree(pv: *mut c_void) {
    if OLE32_INIT.load(Ordering::Acquire) != 2 { load_ole32_fns(); }
    let f = FN_CO_TASK_MEM_FREE;
    if f != 0 {
        let func: unsafe extern "system" fn(*mut c_void) = core::mem::transmute(f);
        func(pv);
    }
}

unsafe extern "system" fn compat_CoCreateFreeThreadedMarshaler(
    punk_outer: *mut c_void,
    ppunk: *mut *mut c_void,
) -> i32 {
    if OLE32_INIT.load(Ordering::Acquire) != 2 { load_ole32_fns(); }
    let f = FN_CO_CREATE_FTM;
    if f != 0 {
        let func: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32
            = core::mem::transmute(f);
        func(punk_outer, ppunk)
    } else {
        -0x7FFF_FFFF // E_UNEXPECTED
    }
}

#[used] #[no_mangle]
pub static __imp_CoTaskMemFree:
    unsafe extern "system" fn(*mut c_void)
    = compat_CoTaskMemFree;

#[used] #[no_mangle]
pub static __imp_CoCreateFreeThreadedMarshaler:
    unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32
    = compat_CoCreateFreeThreadedMarshaler;
