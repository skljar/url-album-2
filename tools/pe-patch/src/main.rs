// PE binary patcher for Win7 compatibility.
//
// Root cause: Rust 1.78+ libstd dropped Win7 support and now imports
// GetSystemTimePreciseAsFileTime (Win8+) from kernel32.dll as a hard entry in
// the PE import table. Win7's loader reads that table at process startup, cannot
// find the function in its kernel32.dll, and terminates the process before a
// single line of user code runs — so our __imp_* shim in compat.rs never fires.
//
// Fix: rename that one IMAGE_IMPORT_BY_NAME entry to GetSystemTimeAsFileTime
// (available since Windows 2000). Win7's loader resolves it successfully and
// writes its address into the IAT slot. Actual calls still go through the
// __imp_GetSystemTimePreciseAsFileTime static in compat.rs, which points to our
// Win7 shim — the IAT slot value is never read by running code.
//
// Steps performed:
//   1. Validate the file is a recognisable PE.
//   2. Confirm the problematic import is present (exit if already patched).
//   3. Create a timestamped backup in tools/pe-patch/backups/ before any write.
//   4. Locate the name string in raw bytes — must appear exactly once.
//   5. Zero the 2-byte HINT field preceding the name (0x027F is wrong after rename;
//      hint=0 forces the loader to resolve by name, which is always correct).
//   6. Overwrite the 30-byte name field: 23 new bytes + 7 zero-padding bytes.
//      PE loader reads name as null-terminated C-string, so it stops at byte 23.
//   7. Zero the PE Optional Header CheckSum field. Windows loader ignores it for
//      normal application .exe files; zeroing avoids stale-checksum AV alerts.
//   8. Write the patched binary back over the original path.
//
// Usage (PowerShell, from project root):
//   cargo run --manifest-path tools\pe-patch\Cargo.toml -- `
//       target\i686-pc-windows-msvc\release\url-album-3.exe

use std::{fs, path::PathBuf, process};

use chrono::Local;
use object::{File as ObjFile, Object};

// Timestamped backups are stored here, keyed to the crate's source root.
const BACKUP_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), r"\backups");

// The Win8+ function name currently in the PE import table (30 bytes, no NUL).
const OLD: &[u8] = b"GetSystemTimePreciseAsFileTime";
// The Win7-compatible replacement (23 bytes); zero-padded to 30 in the INT slot.
const NEW: &[u8] = b"GetSystemTimeAsFileTime";

fn main() {
    let path: PathBuf = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: pe-patch <path-to-exe>");
        process::exit(1);
    }).into();

    let mut data = fs::read(&path).expect("cannot read input file");

    // ── 1. Validate: must be a recognisable PE ───────────────────────────────
    ObjFile::parse(&*data).unwrap_or_else(|e| {
        eprintln!("Not a valid PE file: {e}");
        process::exit(1);
    });

    // ── 2. Confirm the import is present ─────────────────────────────────────
    {
        let pe = ObjFile::parse(&*data).unwrap();
        // Temporary borrow; dropped at end of block so `data` is free to mutate.
        if !pe.imports().unwrap_or_default().iter().any(|i| i.name() == OLD) {
            eprintln!(
                "'{}' not found in import table — already patched or wrong binary?",
                s(OLD)
            );
            process::exit(1);
        }
    }
    println!("Confirmed: '{}' present in import table", s(OLD));

    // ── 3. Timestamped backup before any modification ─────────────────────────
    let exe_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let ts = Local::now().format("%Y%m%d-%H%M%S");
    let backup_dir = PathBuf::from(BACKUP_DIR);
    fs::create_dir_all(&backup_dir).expect("cannot create backup directory");
    let backup_path = backup_dir.join(format!("{exe_name}-{ts}.backup"));
    fs::copy(&path, &backup_path).expect("cannot write backup");
    println!("Backup:   {}", backup_path.display());

    // ── 4. Locate name in raw bytes — exactly one occurrence required ─────────
    let hits: Vec<usize> = data
        .windows(OLD.len())
        .enumerate()
        .filter(|(_, w)| *w == OLD)
        .map(|(i, _)| i)
        .collect();

    let name_off = match hits.len() {
        1 => hits[0],
        0 => {
            eprintln!("String not found in raw bytes (import-table / raw-search mismatch)");
            process::exit(1);
        }
        n => {
            eprintln!("Found {n} occurrences — unsafe to patch automatically");
            process::exit(1);
        }
    };

    // IMAGE_IMPORT_BY_NAME: [WORD Hint | CHAR Name[]]
    // The hint occupies the 2 bytes immediately before the name string.
    let hint_off = name_off.checked_sub(2).unwrap_or_else(|| {
        eprintln!("Hint offset underflow — unexpected binary layout");
        process::exit(1);
    });

    let old_hint = u16::from_le_bytes([data[hint_off], data[hint_off + 1]]);
    println!(
        "Patching: '{}' ({} bytes) → '{}' ({} bytes + {} NUL pad)",
        s(OLD), OLD.len(),
        s(NEW), NEW.len(),
        OLD.len() - NEW.len()
    );
    println!("  hint   @ 0x{hint_off:08X} : 0x{old_hint:04X} → 0x0000  (name-lookup forced)");
    println!("  name   @ 0x{name_off:08X} : in-place overwrite");

    // ── 5. Zero the hint field ───────────────────────────────────────────────
    data[hint_off]     = 0;
    data[hint_off + 1] = 0;

    // ── 6. Replace name, zero-pad tail to preserve 30-byte block size ────────
    // After: b"GetSystemTimeAsFileTime\0\0\0\0\0\0\0"  (23 name + 7 zeros = 30)
    for i in 0..OLD.len() {
        data[name_off + i] = NEW.get(i).copied().unwrap_or(0);
    }

    // ── 7. Zero the PE Optional Header checksum ───────────────────────────────
    match pe_checksum_offset(&data) {
        Some(off) => {
            let old_cs = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            println!("  checksum @ 0x{off:08X} : 0x{old_cs:08X} → 0x00000000");
            data[off..off + 4].fill(0);
        }
        None => eprintln!("Warning: PE checksum field not located — skipping"),
    }

    // ── 8. Redirect bcryptprimitives.dll → CRYPTBASE.dll in delay-load table ──
    if !patch_delay_bcrypt(&mut data) {
        eprintln!("Warning: delay-load patch for bcryptprimitives.dll failed or not needed");
    }

    // ── 9. Patch synch IAT: WaitOnAddress/WakeByAddress* → compat shims ─────
    if !patch_synch_iat(&mut data) {
        eprintln!("Warning: synch IAT patch failed or not needed");
    }

    // ── 10. Patch combase IAT: CoTaskMemFree/CoCreateFTM → compat shims ─────
    if !patch_combase_iat(&mut data) {
        eprintln!("Warning: combase IAT patch failed or not needed");
    }

    // ── 11. Write patched binary ──────────────────────────────────────────────
    fs::write(&path, &data).expect("cannot write patched file");
    println!("Done:     {}", path.display());
}

/// Returns the file offset of IMAGE_OPTIONAL_HEADER32.CheckSum.
///
/// PE file layout from byte 0:
///   DOS header  →  e_lfanew (DWORD at offset 0x3C)
///   → "PE\0\0"  (4 bytes, PE signature)
///   → COFF File Header  (20 bytes)
///   → Optional Header   (CheckSum at byte offset 64 within it)
///
/// Combined offset from e_lfanew: 4 + 20 + 64 = 88.
fn pe_checksum_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 0x40 {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(data[0x3C..0x40].try_into().ok()?) as usize;
    if data.get(e_lfanew..e_lfanew + 4)? != b"PE\0\0" {
        return None;
    }
    Some(e_lfanew + 88)
}

// ── PE structural helpers ────────────────────────────────────────────────────

fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn write_u32(data: &mut [u8], off: usize, val: u32) {
    data[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

/// Convert RVA → file offset using the section table.
/// Returns None if the RVA isn't covered by any section.
fn rva_to_off(data: &[u8], rva: u32) -> Option<usize> {
    // e_lfanew → PE signature → COFF header (20 bytes) → Optional Header → sections
    let e_lfanew = read_u32(data, 0x3C) as usize;
    // COFF SizeOfOptionalHeader at PE+20
    let opt_size = u16::from_le_bytes(data[e_lfanew + 20..e_lfanew + 22].try_into().ok()?) as usize;
    // Number of sections at PE+6
    let num_sec  = u16::from_le_bytes(data[e_lfanew + 6..e_lfanew + 8].try_into().ok()?) as usize;
    // First section header: PE sig(4) + COFF(20) + optional header
    let sec_base = e_lfanew + 4 + 20 + opt_size;
    for i in 0..num_sec {
        let sh = sec_base + i * 40;
        let va  = read_u32(data, sh + 12); // VirtualAddress
        let vsz = read_u32(data, sh + 16); // VirtualSize (or SizeOfRawData as fallback)
        let raw = read_u32(data, sh + 20); // PointerToRawData
        if rva >= va && rva < va + vsz {
            return Some((rva - va + raw) as usize);
        }
    }
    None
}

/// Patch bcryptprimitives.dll → CRYPTBASE.dll in the delay-load directory,
/// and redirect ProcessPrng (by name) → SystemFunction036 (ordinal 9).
///
/// Why only INT, not IAT:
///   The delay-load IAT slot holds the address of `_tailMerge_bcryptprimitives_dll`
///   (the thunk in .text). At runtime the thunk calls __delayLoadHelper2, which does
///   LoadLibrary + GetProcAddress using the INT entry, then writes the resolved
///   function address *into* the IAT slot — overwriting whatever is there.
///   If we wrote 0x80000009 into the IAT slot now, the thunk would try to jump to
///   virtual address 0x9 before it ever runs __delayLoadHelper2 → instant AV.
///   Only the INT entry and the DLL name string need to change.
fn patch_delay_bcrypt(data: &mut Vec<u8>) -> bool {
    // IMAGE_OPTIONAL_HEADER32: DataDirectory at offset 96 from start of optional header.
    // Each DataDirectory entry = 8 bytes (RVA + Size). Directory index 13 = Delay Import.
    let e_lfanew = read_u32(data, 0x3C) as usize;
    // Optional header starts at e_lfanew + 4 (PE sig) + 20 (COFF) = e_lfanew + 24
    let opt_off = e_lfanew + 24;
    // DataDirectory[13] = opt_off + 96 + 13*8
    let dd13_off = opt_off + 96 + 13 * 8;
    let dir_rva  = read_u32(data, dd13_off);
    let dir_size = read_u32(data, dd13_off + 4);
    if dir_rva == 0 || dir_size == 0 {
        eprintln!("No delay-load directory in PE");
        return false;
    }

    // ImgDelayDescr (32 bytes each, PE32 variant without grAttrs bit set uses RVAs):
    //   +0  grAttrs       (1 = RVA-based, 0 = VA-based — old format)
    //   +4  rvaDLLName    RVA of DLL name string
    //   +8  rvaHmod       RVA of HMODULE slot
    //   +12 rvaIAT        RVA of IAT array
    //   +16 rvaINT        RVA of INT array
    //   +20 rvaBoundIAT   RVA of bound IAT (optional)
    //   +24 rvaUnloadIAT  RVA of unload IAT (optional)
    //   +28 dwTimeStamp
    let BCRYPT = b"bcryptprimitives.dll";
    let CBASE  = b"CRYPTBASE.dll";
    // IMAGE_ORDINAL_FLAG32 | 9  — imports SystemFunction036 (= RtlGenRandom) by ordinal.
    // CRYPTBASE!SystemFunction036 is ordinal 9 on Win7 and all later Windows.
    // ABI matches ProcessPrng on i686: stdcall (PVOID buf, ULONG len) -> BOOL.
    const ORD_FLAG: u32 = 0x8000_0000;
    const SYS036_ORD: u32 = ORD_FLAG | 9;

    let num_entries = dir_size as usize / 32;
    let dir_off = match rva_to_off(data, dir_rva) {
        Some(o) => o,
        None => { eprintln!("delay-load directory RVA not mapped"); return false; }
    };

    for i in 0..num_entries {
        let d = dir_off + i * 32;
        let dll_name_rva = read_u32(data, d + 4);
        if dll_name_rva == 0 { break; } // sentinel entry

        let dll_name_off = match rva_to_off(data, dll_name_rva) {
            Some(o) => o,
            None    => continue,
        };
        // Case-insensitive compare against "bcryptprimitives.dll"
        let slice = &data[dll_name_off..dll_name_off + BCRYPT.len()];
        let matches = slice.iter().zip(BCRYPT.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase());
        if !matches { continue; }

        println!("Found bcryptprimitives.dll delay-load descriptor at entry {i}");

        // Walk INT to find ProcessPrng (imported by name, not ordinal).
        // INT entry = 4 bytes RVA of IMAGE_IMPORT_BY_NAME {Hint:u16, Name:[]}.
        let int_rva = read_u32(data, d + 16);
        let int_off = match rva_to_off(data, int_rva) {
            Some(o) => o,
            None    => { eprintln!("INT RVA not mapped"); return false; }
        };

        let mut int_idx = 0usize;
        loop {
            let entry = read_u32(data, int_off + int_idx * 4);
            if entry == 0 { eprintln!("ProcessPrng not found in INT"); return false; }
            if entry & ORD_FLAG != 0 { int_idx += 1; continue; } // already ordinal

            let ibn_off = match rva_to_off(data, entry) {
                Some(o) => o,
                None    => { int_idx += 1; continue; }
            };
            // IMAGE_IMPORT_BY_NAME: [Hint:u16][Name:char[]]
            let name_off = ibn_off + 2; // skip Hint
            let name = b"ProcessPrng";
            if data[name_off..name_off + name.len()] != *name {
                int_idx += 1;
                continue;
            }

            println!("  Found ProcessPrng at INT[{int_idx}] (RVA 0x{entry:08X})");

            // ── Patch 1: redirect INT entry to ordinal 9 of CRYPTBASE.dll ──────
            // This is the only field __delayLoadHelper2 reads to resolve the import.
            // We do NOT touch the IAT slot — it holds the `_tailMerge_*` thunk address
            // and will be overwritten by the thunk itself after successful resolution.
            write_u32(data, int_off + int_idx * 4, SYS036_ORD);
            println!("  INT[{int_idx}] → 0x{SYS036_ORD:08X} (IMAGE_ORDINAL_FLAG32 | 9 = CRYPTBASE!SystemFunction036)");
            break;
        }

        // ── Patch 2: rename DLL string to CRYPTBASE.dll ──────────────────────
        // The string buffer in .rdata is large enough (bcryptprimitives.dll = 20 chars,
        // CRYPTBASE.dll = 13 chars). Zero-fill the remainder so no stale bytes remain.
        data[dll_name_off..dll_name_off + BCRYPT.len() + 1].fill(0);
        data[dll_name_off..dll_name_off + CBASE.len()].copy_from_slice(CBASE);
        data[dll_name_off + CBASE.len()] = 0; // explicit null terminator
        println!("  DLL name → \"CRYPTBASE.dll\"");

        return true;
    }

    eprintln!("bcryptprimitives.dll not found in delay-load directory");
    false
}

/// Reads the exe's export table and returns VA (image_base + RVA) of a named function.
fn find_export_va(data: &[u8], target: &[u8]) -> Option<u32> {
    let e_lfanew   = read_u32(data, 0x3C) as usize;
    let image_base = read_u32(data, e_lfanew + 24 + 28); // PE32 Optional Header: ImageBase
    let opt_off    = e_lfanew + 24;
    let exp_rva    = read_u32(data, opt_off + 96);        // DataDirectory[0].VirtualAddress
    if exp_rva == 0 { return None; }
    let exp_off = rva_to_off(data, exp_rva)?;

    let num_names = read_u32(data, exp_off + 24) as usize;
    let funcs_off = rva_to_off(data, read_u32(data, exp_off + 28))?;
    let names_off = rva_to_off(data, read_u32(data, exp_off + 32))?;
    let ords_off  = rva_to_off(data, read_u32(data, exp_off + 36))?;

    for i in 0..num_names {
        let name_off = rva_to_off(data, read_u32(data, names_off + i * 4))?;
        let len = target.len();
        if name_off + len + 1 > data.len() { continue; }
        if &data[name_off..name_off + len] == target && data[name_off + len] == 0 {
            let ord = u16::from_le_bytes(
                data[ords_off + i * 2..ords_off + i * 2 + 2].try_into().ok()?
            ) as usize;
            return Some(image_base + read_u32(data, funcs_off + ord * 4));
        }
    }
    None
}

/// Patches the delay-load IAT for api-ms-win-core-synch-l1-2-0.dll so that
/// WaitOnAddress / WakeByAddressAll / WakeByAddressSingle jump directly to
/// the compat shims built into the exe, bypassing the delay-load thunk entirely.
///
/// Why IAT (not INT) here, unlike the bcrypt patch:
///   For bcrypt we still wanted __delayLoadHelper2 to run — it does LoadLibrary
///   ("CRYPTBASE.dll") + GetProcAddress(ord 9) at runtime.  For the synch APIs
///   there is no Win7 DLL to load; we want the thunk to NEVER fire.
///   Pre-patching the IAT slot with our shim VA means the first CALL [IAT] jumps
///   straight to the shim — __delayLoadHelper2 and LoadLibrary are never reached.
///   The Windows loader does NOT touch the delay-load IAT at startup (only the
///   regular import IAT is processed by the loader), so our patch survives.
fn patch_synch_iat(data: &mut Vec<u8>) -> bool {
    const SHIMS: [(&[u8], &[u8]); 3] = [
        (b"WaitOnAddress",       b"compat_WaitOnAddress"),
        (b"WakeByAddressAll",    b"compat_WakeByAddressAll"),
        (b"WakeByAddressSingle", b"compat_WakeByAddressSingle"),
    ];

    // ── Resolve shim VAs from the exe export table ───────────────────────────
    let mut shim_vas = [0u32; 3];
    for (i, (_, shim_name)) in SHIMS.iter().enumerate() {
        match find_export_va(data, shim_name) {
            Some(va) => {
                println!("  Export {}: VA=0x{va:08X}", s(shim_name));
                shim_vas[i] = va;
            }
            None => {
                eprintln!("Export '{}' not found — /EXPORT: missing in build.rs?", s(shim_name));
                return false;
            }
        }
    }

    // ── Find delay-load descriptor for api-ms-win-core-synch-l1-2-0.dll ─────
    let e_lfanew   = read_u32(data, 0x3C) as usize;
    let image_base = read_u32(data, e_lfanew + 24 + 28);
    let opt_off    = e_lfanew + 24;
    let dd13_off   = opt_off + 96 + 13 * 8;
    let dir_rva    = read_u32(data, dd13_off);
    let dir_size   = read_u32(data, dd13_off + 4);
    if dir_rva == 0 || dir_size == 0 {
        eprintln!("No delay-load directory in PE");
        return false;
    }
    let dir_off = match rva_to_off(data, dir_rva) {
        Some(o) => o,
        None => { eprintln!("Delay-load directory RVA not mapped"); return false; }
    };

    const SYNCH_DLL: &[u8] = b"api-ms-win-core-synch-l1-2-0.dll";
    let num_entries = dir_size as usize / 32;

    for i in 0..num_entries {
        let d = dir_off + i * 32;
        let dll_name_rva = read_u32(data, d + 4);
        if dll_name_rva == 0 { break; }
        let dll_name_off = match rva_to_off(data, dll_name_rva) {
            Some(o) => o,
            None => continue,
        };
        let n = SYNCH_DLL.len();
        if dll_name_off + n + 1 > data.len() { continue; }
        let matches = data[dll_name_off..dll_name_off + n].iter().zip(SYNCH_DLL)
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
            && data[dll_name_off + n] == 0;
        if !matches { continue; }

        println!("Found api-ms-win-core-synch-l1-2-0.dll delay-load descriptor at entry {i}");

        let int_off = match rva_to_off(data, read_u32(data, d + 16)) {
            Some(o) => o,
            None => { eprintln!("INT RVA not mapped"); return false; }
        };
        let iat_base = match rva_to_off(data, read_u32(data, d + 12)) {
            Some(o) => o,
            None => { eprintln!("IAT RVA not mapped"); return false; }
        };

        let mut patched = 0usize;
        let mut idx = 0usize;
        loop {
            let int_entry = read_u32(data, int_off + idx * 4);
            if int_entry == 0 { break; }

            if int_entry & 0x8000_0000 == 0 { // import by name
                if let Some(ibn_off) = rva_to_off(data, int_entry) {
                    let name_off = ibn_off + 2; // skip Hint word
                    for (api_idx, (api_name, _)) in SHIMS.iter().enumerate() {
                        let len = api_name.len();
                        if name_off + len + 1 > data.len() { continue; }
                        if &data[name_off..name_off + len] != *api_name
                            || data[name_off + len] != 0 { continue; }

                        let iat_slot = iat_base + idx * 4;
                        let current  = read_u32(data, iat_slot);
                        let target   = shim_vas[api_idx];

                        if current == target {
                            println!("  IAT[{idx}] {} — already patched, skip", s(api_name));
                        } else {
                            // Safety: current should be a thunk VA within the exe image
                            let exe_end = image_base.saturating_add(data.len() as u32);
                            if current < image_base || current >= exe_end {
                                eprintln!("  Warning: IAT[{idx}] {} has unexpected value \
                                           0x{current:08X} (not a thunk VA — PE struct changed?)",
                                    s(api_name));
                            }
                            write_u32(data, iat_slot, target);
                            println!("  IAT[{idx}] {} : 0x{current:08X} (thunk) \
                                      → 0x{target:08X} (compat shim)", s(api_name));
                        }
                        patched += 1;
                        break;
                    }
                }
            }
            idx += 1;
        }

        if patched == 0 {
            eprintln!("No WaitOnAddress / WakeByAddress* entries found in INT");
            return false;
        }
        println!("  Synch IAT: {patched}/3 entries patched");
        return true;
    }

    eprintln!("api-ms-win-core-synch-l1-2-0.dll not found in delay-load directory");
    false
}

/// Patches the delay-load IAT for combase.dll so that CoTaskMemFree and
/// CoCreateFreeThreadedMarshaler jump directly to the compat shims in the exe.
///
/// combase.dll does not exist on Win7 (those functions live in ole32.dll there).
/// Same IAT-bypass technique as patch_synch_iat: pre-patch the IAT slot with the
/// shim VA so the first CALL [IAT] goes straight to the shim, never reaching the
/// delay-load thunk or LoadLibrary("combase.dll").
fn patch_combase_iat(data: &mut Vec<u8>) -> bool {
    const SHIMS: [(&[u8], &[u8]); 2] = [
        (b"CoTaskMemFree",                b"compat_CoTaskMemFree"),
        (b"CoCreateFreeThreadedMarshaler", b"compat_CoCreateFreeThreadedMarshaler"),
    ];

    let mut shim_vas = [0u32; 2];
    for (i, (_, shim_name)) in SHIMS.iter().enumerate() {
        match find_export_va(data, shim_name) {
            Some(va) => {
                println!("  Export {}: VA=0x{va:08X}", s(shim_name));
                shim_vas[i] = va;
            }
            None => {
                eprintln!("Export '{}' not found — /EXPORT: missing in build.rs?", s(shim_name));
                return false;
            }
        }
    }

    let e_lfanew   = read_u32(data, 0x3C) as usize;
    let image_base = read_u32(data, e_lfanew + 24 + 28);
    let opt_off    = e_lfanew + 24;
    let dd13_off   = opt_off + 96 + 13 * 8;
    let dir_rva    = read_u32(data, dd13_off);
    let dir_size   = read_u32(data, dd13_off + 4);
    if dir_rva == 0 || dir_size == 0 {
        eprintln!("No delay-load directory in PE");
        return false;
    }
    let dir_off = match rva_to_off(data, dir_rva) {
        Some(o) => o,
        None => { eprintln!("Delay-load directory RVA not mapped"); return false; }
    };

    const COMBASE_DLL: &[u8] = b"combase.dll";
    let num_entries = dir_size as usize / 32;

    for i in 0..num_entries {
        let d = dir_off + i * 32;
        let dll_name_rva = read_u32(data, d + 4);
        if dll_name_rva == 0 { break; }
        let dll_name_off = match rva_to_off(data, dll_name_rva) {
            Some(o) => o,
            None => continue,
        };
        let n = COMBASE_DLL.len();
        if dll_name_off + n + 1 > data.len() { continue; }
        let matches = data[dll_name_off..dll_name_off + n].iter().zip(COMBASE_DLL)
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
            && data[dll_name_off + n] == 0;
        if !matches { continue; }

        println!("Found combase.dll delay-load descriptor at entry {i}");

        let int_off = match rva_to_off(data, read_u32(data, d + 16)) {
            Some(o) => o,
            None => { eprintln!("INT RVA not mapped"); return false; }
        };
        let iat_base = match rva_to_off(data, read_u32(data, d + 12)) {
            Some(o) => o,
            None => { eprintln!("IAT RVA not mapped"); return false; }
        };

        let mut patched = 0usize;
        let mut idx = 0usize;
        loop {
            let int_entry = read_u32(data, int_off + idx * 4);
            if int_entry == 0 { break; }

            if int_entry & 0x8000_0000 == 0 {
                if let Some(ibn_off) = rva_to_off(data, int_entry) {
                    let name_off = ibn_off + 2;
                    for (api_idx, (api_name, _)) in SHIMS.iter().enumerate() {
                        let len = api_name.len();
                        if name_off + len + 1 > data.len() { continue; }
                        if &data[name_off..name_off + len] != *api_name
                            || data[name_off + len] != 0 { continue; }

                        let iat_slot = iat_base + idx * 4;
                        let current  = read_u32(data, iat_slot);
                        let target   = shim_vas[api_idx];

                        if current == target {
                            println!("  IAT[{idx}] {} — already patched, skip", s(api_name));
                        } else {
                            let exe_end = image_base.saturating_add(data.len() as u32);
                            if current < image_base || current >= exe_end {
                                eprintln!("  Warning: IAT[{idx}] {} has unexpected value \
                                           0x{current:08X} (not a thunk VA — PE struct changed?)",
                                    s(api_name));
                            }
                            write_u32(data, iat_slot, target);
                            println!("  IAT[{idx}] {} : 0x{current:08X} (thunk) \
                                      → 0x{target:08X} (compat shim)", s(api_name));
                        }
                        patched += 1;
                        break;
                    }
                }
            }
            idx += 1;
        }

        if patched == 0 {
            eprintln!("No CoTaskMemFree / CoCreateFreeThreadedMarshaler entries found in INT");
            return false;
        }
        println!("  Combase IAT: {patched}/2 entries patched");
        return true;
    }

    eprintln!("combase.dll not found in delay-load directory");
    false
}

fn s(b: &[u8]) -> &str {
    std::str::from_utf8(b).unwrap_or("<non-UTF8>")
}
