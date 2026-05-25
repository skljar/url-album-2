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

    // ── 8. Write patched binary ───────────────────────────────────────────────
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

fn s(b: &[u8]) -> &str {
    std::str::from_utf8(b).unwrap_or("<non-UTF8>")
}
