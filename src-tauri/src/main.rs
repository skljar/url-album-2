#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod db;
mod importer;

use std::sync::Mutex;
use rusqlite::Connection;
use tauri::Manager;

struct AppState {
    db: Mutex<Connection>,
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn get_tree(state: tauri::State<AppState>) -> Result<Vec<db::TreeNode>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_tree(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_bookmarks(
    state: tauri::State<AppState>,
    folder_id: i64,
) -> Result<Vec<db::Bookmark>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_bookmarks(&conn, folder_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn is_empty(state: tauri::State<AppState>) -> bool {
    state
        .db
        .lock()
        .map(|conn| db::is_empty(&conn))
        .unwrap_or(true)
}

/// Try to locate ua.dat or ua.dat.bak next to the executable.
#[tauri::command]
fn find_uadat() -> Option<String> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for name in ["ua.dat", "ua.dat.bak"] {
        let p = exe_dir.join(name);
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

#[tauri::command]
fn import_uadat(state: tauri::State<AppState>, path: String) -> Result<usize, String> {
    let raw = std::fs::read(&path).map_err(|e| e.to_string())?;
    // The original file is Windows-1251 encoded
    let (text, _, _) = encoding_rs::WINDOWS_1251.decode(&raw);

    // Thumbnails live in a "Data" subfolder next to the dat file
    let data_dir = std::path::Path::new(&path)
        .parent()
        .map(|p| p.join("Data").to_string_lossy().into_owned())
        .unwrap_or_default();

    let nodes = importer::parse(&text);
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import(&conn, &nodes, &data_dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_node(state: tauri::State<AppState>, id: i64, title: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE nodes SET title = ?1 WHERE id = ?2", rusqlite::params![title, id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_folder(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // Use parameter binding — execute_batch doesn't support params for multi-statement,
    // so we use a single DELETE with a recursive CTE via a prepared statement.
    let mut stmt = conn.prepare(
        "WITH RECURSIVE sub(id) AS (
             VALUES(?1)
             UNION ALL
             SELECT n.id FROM nodes n JOIN sub s ON n.parent = s.id
         )
         DELETE FROM nodes WHERE id IN (SELECT id FROM sub)"
    ).map_err(|e| e.to_string())?;
    stmt.execute(rusqlite::params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn clear_thumb(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE nodes SET thumb = NULL WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn refresh_thumb(state: tauri::State<AppState>, id: i64, url: String) -> Result<String, String> {
    let data_dir = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent().ok_or("no parent dir")?
        .join("Data");
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = data_dir.join(format!("{ts}.png"));
    let path_str = path.to_string_lossy().into_owned();

    // Try Edge, then Chrome (headless --screenshot mode)
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    ];
    let browser = candidates.iter()
        .find(|p| std::path::Path::new(p).exists())
        .ok_or("Edge или Chrome не найден")?;

    let tmp_dir = std::env::temp_dir().join("ua_screenshot_profile");
    let status = std::process::Command::new(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--hide-scrollbars",
            "--window-size=1280,800",
            &format!("--user-data-dir={}", tmp_dir.display()),
            &format!("--screenshot={path_str}"),
            &url,
        ])
        .status()
        .map_err(|e| e.to_string())?;

    if !status.success() || !path.exists() {
        return Err("Не удалось создать скриншот".to_string());
    }

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE nodes SET thumb = ?1 WHERE id = ?2",
        rusqlite::params![path_str, id],
    ).map_err(|e| e.to_string())?;

    Ok(path_str)
}

#[tauri::command]
fn delete_node(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM nodes WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn update_bookmark(
    state: tauri::State<AppState>,
    id: i64, title: String, url: String, note: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let note_val: Option<String> = if note.trim().is_empty() { None } else { Some(note) };
    conn.execute(
        "UPDATE nodes SET title = ?1, url = ?2, note = ?3 WHERE id = ?4",
        rusqlite::params![title, url, note_val, id],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct UrlCheckResult {
    pub url:      String,
    pub status:   u16,
    pub ok:       bool,
    pub timed_out: bool,
    pub redirect: Option<String>,
    pub ms:       u64,
    pub err:      Option<String>,
}

#[tauri::command]
async fn check_url(url: String) -> UrlCheckResult {
    let t0 = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(8);
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("Mozilla/5.0 URL-Album-Checker/1.0")
        .build()
    {
        Ok(c) => c,
        Err(e) => return UrlCheckResult { url, status: 0, ok: false, timed_out: false,
            redirect: None, ms: 0, err: Some(e.to_string()) },
    };
    let resp = match client.head(&url).send().await {
        Ok(r) if r.status().as_u16() == 405 => client.get(&url).send().await,
        other => other,
    };
    let ms = t0.elapsed().as_millis() as u64;
    match resp {
        Ok(r) => {
            let status    = r.status().as_u16();
            let final_url = r.url().to_string();
            UrlCheckResult {
                ok: status < 400, timed_out: false,
                redirect: (final_url != url).then_some(final_url),
                err: None, url, status, ms,
            }
        }
        Err(e) => {
            let timed_out = e.is_timeout();
            UrlCheckResult { url, status: 0, ok: false, timed_out,
                redirect: None, ms, err: Some(e.to_string()) }
        }
    }
}

#[tauri::command]
fn sort_all_bookmarks(state: tauri::State<AppState>, by: String, desc: bool) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let dir = if desc { "DESC" } else { "ASC" };
    let col = match by.as_str() {
        "url"     => "COALESCE(url,'')",
        "created" => "COALESCE(created,'zzzz')",
        _         => "title",
    };
    let folder_ids: Vec<i64> = {
        let mut s = conn.prepare("SELECT id FROM nodes WHERE kind='folder'")
            .map_err(|e| e.to_string())?;
        let x = s.query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(|e| e.to_string())?;
        x
    };
    for fid in folder_ids {
        let sql = format!(
            "SELECT id FROM nodes WHERE parent={fid}
             ORDER BY CASE kind WHEN 'folder' THEN 0 ELSE 1 END, {col} {dir}"
        );
        let ids: Vec<i64> = {
            let mut s = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let x = s.query_map([], |r| r.get::<_, i64>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<i64>>>()
                .map_err(|e| e.to_string())?;
            x
        };
        for (i, id) in ids.iter().enumerate() {
            conn.execute("UPDATE nodes SET sort_idx=?1 WHERE id=?2",
                rusqlite::params![i as i64, id]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn backup_db(window: tauri::Window) -> Result<(), String> {
    let src = std::env::current_exe().map_err(|e| e.to_string())?
        .parent().ok_or("no parent")?.join("album.db");
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Сохранить резервную копию базы")
        .add_filter("База данных", &["db"])
        .save_file().await.ok_or("Отменено")?;
    std::fs::copy(&src, file.path()).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn backup_db_with_data(window: tauri::Window) -> Result<(), String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Выбрать папку для резервной копии")
        .pick_folder().await.ok_or("Отменено")?;
    let dir = file.path().to_path_buf();
    let exe_dir = std::env::current_exe().map_err(|e| e.to_string())?
        .parent().ok_or("no parent")?.to_path_buf();
    std::fs::copy(exe_dir.join("album.db"), dir.join("album.db"))
        .map_err(|e| e.to_string())?;
    let data_src = exe_dir.join("Data");
    if data_src.exists() {
        let data_dst = dir.join("Data");
        std::fs::create_dir_all(&data_dst).map_err(|e| e.to_string())?;
        for e in std::fs::read_dir(&data_src).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            std::fs::copy(e.path(), data_dst.join(e.file_name()))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn sort_folder(
    state: tauri::State<AppState>,
    folder_id: i64,
    by: String,
    desc: bool,
) -> Result<Vec<i64>, String> {
    let conn  = state.db.lock().map_err(|e| e.to_string())?;
    let dir   = if desc { "DESC" } else { "ASC" };
    let col   = match by.as_str() {
        "url"     => "COALESCE(url, '')",
        "created" => "COALESCE(created, 'zzzz')",
        _         => "title",
    };
    // Folders first, then bookmarks, each group sorted by chosen column
    let sql = format!(
        "SELECT id FROM nodes WHERE parent = ?1
         ORDER BY CASE kind WHEN 'folder' THEN 0 ELSE 1 END, {col} {dir}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let ids: Vec<i64> = stmt
        .query_map([folder_id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<i64>>>()
        .map_err(|e| e.to_string())?;
    for (idx, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE nodes SET sort_idx = ?1 WHERE id = ?2",
            rusqlite::params![idx as i64, id],
        ).map_err(|e| e.to_string())?;
    }
    Ok(ids)
}

#[tauri::command]
async fn export_folder_html(state: tauri::State<'_, AppState>, window: tauri::Window, folder_id: i64) -> Result<(), String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Экспорт в HTML")
        .add_filter("HTML файл", &["html", "htm"])
        .save_file().await.ok_or("Отменено")?;
    let content = { let c = state.db.lock().map_err(|e| e.to_string())?; db::export_html(&c, folder_id).map_err(|e| e.to_string())? };
    std::fs::write(file.path(), content.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_folder_txt(state: tauri::State<'_, AppState>, window: tauri::Window, folder_id: i64) -> Result<(), String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Экспорт в TXT")
        .add_filter("Текстовый файл", &["txt"])
        .save_file().await.ok_or("Отменено")?;
    let content = { let c = state.db.lock().map_err(|e| e.to_string())?; db::export_txt(&c, folder_id).map_err(|e| e.to_string())? };
    std::fs::write(file.path(), content.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_folder_sync(state: tauri::State<'_, AppState>, window: tauri::Window, folder_id: i64, with_images: bool) -> Result<(), String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Экспорт файла синхронизации")
        .add_filter("Файл синхронизации", &["json"])
        .save_file().await.ok_or("Отменено")?;
    let content = { let c = state.db.lock().map_err(|e| e.to_string())?; db::export_sync(&c, folder_id, with_images).map_err(|e| e.to_string())? };
    std::fs::write(file.path(), content.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_browser_file(window: tauri::Window) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .add_filter("Программы", &["exe"])
        .set_title("Выберите браузер")
        .pick_file().await
        .map(|f| f.path().to_string_lossy().into_owned())
}

#[tauri::command]
fn update_note(state: tauri::State<AppState>, id: i64, note: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let val: Option<String> = if note.trim().is_empty() { None } else { Some(note) };
    conn.execute("UPDATE nodes SET note = ?1 WHERE id = ?2", rusqlite::params![val, id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn search_bookmarks(
    state: tauri::State<AppState>,
    query: String,
    by_title: Option<bool>,
    by_url:   Option<bool>,
    by_note:  Option<bool>,
) -> Result<Vec<db::SearchResult>, String> {
    if query.trim().is_empty() { return Ok(vec![]); }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::search_bookmarks(
        &conn, &query,
        by_title.unwrap_or(true),
        by_url  .unwrap_or(true),
        by_note .unwrap_or(true),
    ).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_stats(state: tauri::State<AppState>) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let total: i64    = conn.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0)).unwrap_or(0);
    let folders: i64  = conn.query_row("SELECT COUNT(*) FROM nodes WHERE kind='folder'", [], |r| r.get(0)).unwrap_or(0);
    let books: i64    = conn.query_row("SELECT COUNT(*) FROM nodes WHERE kind='bookmark'", [], |r| r.get(0)).unwrap_or(0);
    let orphans: i64  = conn.query_row("SELECT COUNT(*) FROM nodes WHERE kind='bookmark' AND parent IS NULL", [], |r| r.get(0)).unwrap_or(0);
    Ok(format!("total={total} folders={folders} bookmarks={books} orphan_bookmarks={orphans}"))
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    #[cfg(windows)]
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &url])
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&url).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_url_with(url: String, browser: String) -> Result<(), String> {
    if browser == "default" {
        return open_url(url);
    }
    std::process::Command::new(&browser)
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Generic file save ────────────────────────────────────────────────────────

#[tauri::command]
async fn save_text_file(window: tauri::Window, content: String, default_name: Option<String>) -> Result<(), String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Сохранить файл")
        .add_filter("Текстовый файл", &["txt"])
        .set_file_name(default_name.as_deref().unwrap_or("export.txt"))
        .save_file().await.ok_or("Отменено")?;
    std::fs::write(file.path(), content.as_bytes()).map_err(|e| e.to_string())
}

// ── Settings (portable) ──────────────────────────────────────────────────────

#[tauri::command]
fn load_settings() -> String {
    std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.join("settings.json")))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn save_settings(json: String) -> Result<(), String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?
        .parent().ok_or("no parent")?.join("settings.json");
    std::fs::write(path, json.as_bytes()).map_err(|e| e.to_string())
}

// ── Toolbar config (portable) ────────────────────────────────────────────────

#[tauri::command]
fn load_toolbar_config() -> String {
    std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.join("toolbar.json")))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn save_toolbar_config(json: String) -> Result<(), String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?
        .parent().ok_or("no parent")?.join("toolbar.json");
    std::fs::write(path, json.as_bytes()).map_err(|e| e.to_string())
}

// ── Sort index ───────────────────────────────────────────────────────────────

#[tauri::command]
fn set_sort_idx(state: tauri::State<AppState>, id: i64, sort_idx: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE nodes SET sort_idx = ?1 WHERE id = ?2", rusqlite::params![sort_idx, id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Create nodes ─────────────────────────────────────────────────────────────

#[tauri::command]
fn create_folder(state: tauri::State<AppState>, parent_id: Option<i64>, title: String) -> Result<i64, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // IS works with both NULL and integer in SQLite
    let max: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_idx),-1) FROM nodes WHERE parent IS ?1",
        rusqlite::params![parent_id], |r| r.get(0)
    ).unwrap_or(-1);
    conn.execute("INSERT INTO nodes (parent,kind,title,sort_idx) VALUES(?1,'folder',?2,?3)",
        rusqlite::params![parent_id, title, max + 1]).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
fn create_bookmark(state: tauri::State<AppState>, parent_id: i64, title: String, url: String) -> Result<i64, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let max: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_idx),-1) FROM nodes WHERE parent=?1", [parent_id], |r| r.get(0)
    ).unwrap_or(-1);
    conn.execute("INSERT INTO nodes (parent,kind,title,url,sort_idx) VALUES(?1,'bookmark',?2,?3,?4)",
        rusqlite::params![parent_id, title, url, max+1]).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

// ── Browser detection ────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
struct DetectedBrowser {
    id:             String,
    name:           String,
    kind:           String, // "chromium" | "firefox"
    bookmarks_path: String,
}

#[derive(serde::Serialize)]
struct ImportSummary { links: usize, folders: usize }


fn chromium_bookmarks_paths(base: &str, app_name: &str) -> Vec<String> {
    // Standard Chromium profile layouts
    vec![
        format!("{}\\{}\\User Data\\Default\\Bookmarks", base, app_name),
        format!("{}\\{}\\Default\\Bookmarks", base, app_name),
        format!("{}\\{}\\Bookmarks", base, app_name),
    ]
}

fn detect_browsers_list() -> Vec<DetectedBrowser> {
    let local   = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    let pf      = std::env::var("PROGRAMFILES").unwrap_or_default();
    let pf86    = std::env::var("PROGRAMFILES(X86)").unwrap_or_else(|_| pf.clone());
    let mut out: Vec<DetectedBrowser> = Vec::new();

    // ── Chromium-based (LOCALAPPDATA) ──
    let local_apps: &[(&str, &str, &str)] = &[
        ("chrome",   "Google Chrome",  "Google\\Chrome"),
        ("edge",     "Microsoft Edge", "Microsoft\\Edge"),
        ("brave",    "Brave",          "BraveSoftware\\Brave-Browser"),
        ("vivaldi",  "Vivaldi",        "Vivaldi"),
        ("chromium", "Chromium",       "Chromium"),
    ];
    for (id, name, rel) in local_apps {
        for base in &[&local as &str, &pf, &pf86] {
            for path in chromium_bookmarks_paths(base, rel) {
                if exe_exists(&path) && !out.iter().any(|b| b.id == *id) {
                    out.push(DetectedBrowser { id: id.to_string(), name: name.to_string(),
                        kind: "chromium".to_string(), bookmarks_path: path });
                    break;
                }
            }
        }
    }

    // ── Opera family — scan entire Opera Software folder (APPDATA & LOCALAPPDATA) ──
    for base in &[&roaming as &str, &local] {
        let opera_base = format!("{}\\Opera Software", base);
        if let Ok(entries) = std::fs::read_dir(&opera_base) {
            for entry in entries.filter_map(|e| e.ok()) {
                if !entry.path().is_dir() { continue; }
                // Try all three profile layout variants
                let profile_dir = entry.path();
                let candidates = vec![
                    profile_dir.join("Bookmarks"),
                    profile_dir.join("Default").join("Bookmarks"),
                    profile_dir.join("User Data").join("Default").join("Bookmarks"),
                ];
                for bm in candidates {
                    if bm.exists() {
                        let bm_str = bm.to_string_lossy().into_owned();
                        if out.iter().any(|b| b.bookmarks_path == bm_str) { break; }
                        let dir_name = entry.file_name().to_string_lossy().into_owned();
                        let id = format!("opera_{}", out.len());
                        out.push(DetectedBrowser {
                            id, name: dir_name,
                            kind: "chromium".to_string(),
                            bookmarks_path: bm_str,
                        });
                        break;
                    }
                }
            }
        }
    }

    // ── Firefox / Waterfox / LibreWolf etc. (APPDATA) ──
    let ff_bases: &[(&str, &str)] = &[
        ("Mozilla\\Firefox",  "Mozilla Firefox"),
        ("Waterfox",          "Waterfox"),
        ("LibreWolf",         "LibreWolf"),
        ("Pale Moon",         "Pale Moon"),
        ("SeaMonkey",         "SeaMonkey"),
    ];
    for (rel, name) in ff_bases {
        let base = format!("{}\\{}", roaming, rel);
        if let Some(places) = find_gecko_places(&base) {
            let id = rel.replace('\\', "_").to_lowercase();
            if !out.iter().any(|b| b.bookmarks_path == places) {
                out.push(DetectedBrowser { id, name: name.to_string(),
                    kind: "firefox".to_string(), bookmarks_path: places });
            }
        }
    }

    out
}

fn find_gecko_places(browser_base: &str) -> Option<String> {
    // Check profiles.ini for default profile
    let ini_path = format!("{}\\profiles.ini", browser_base);
    let ini = std::fs::read_to_string(&ini_path).ok()?;
    let (mut cur_path, mut cur_rel, mut cur_def) = (String::new(), false, false);
    let mut best: Option<(String, bool)> = None;

    for line in ini.lines().map(str::trim).chain(std::iter::once("[END]")) {
        if line.starts_with('[') {
            if !cur_path.is_empty() {
                let full = if cur_rel {
                    format!("{}\\{}", browser_base, cur_path.replace('/', "\\"))
                } else { cur_path.clone() };
                let places = format!("{}\\places.sqlite", full);
                if std::path::Path::new(&places).exists() {
                    match best {
                        None => { best = Some((places, cur_def)); }
                        Some((_, false)) if cur_def => { best = Some((places, true)); }
                        _ => {}
                    }
                }
            }
            cur_path.clear(); cur_rel = false; cur_def = false;
        } else if let Some(v) = line.strip_prefix("Path=") {
            cur_path = v.trim().to_string();
        } else if line == "Default=1" { cur_def = true; }
          else if line == "IsRelative=1" { cur_rel = true; }
    }
    best.map(|(p, _)| p)
}

// ── Import from arbitrary bookmarks file / profile folder ───────────────────

#[tauri::command]
async fn pick_bookmarks_file(window: tauri::Window) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Выбрать файл закладок")
        .add_filter("Файлы закладок", &["json", "sqlite"])
        .pick_file().await
        .map(|f| f.path().to_string_lossy().into_owned())
}

#[tauri::command]
async fn pick_profile_folder(window: tauri::Window) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Выбрать папку профиля браузера")
        .pick_folder().await
        .map(|f| f.path().to_string_lossy().into_owned())
}

/// Scan a folder for a known bookmarks file. Returns (path, kind) where kind is "chromium"|"firefox".
#[tauri::command]
fn find_bookmarks_in_folder(folder: String) -> Option<serde_json::Value> {
    use serde_json::json;
    let base = std::path::Path::new(&folder);
    // Chromium layout variants
    for rel in &["Bookmarks", "Default\\Bookmarks", "User Data\\Default\\Bookmarks"] {
        let p = base.join(rel);
        if p.exists() {
            return Some(json!({ "path": p.to_string_lossy(), "kind": "chromium" }));
        }
    }
    // Firefox/Gecko layout variants
    for rel in &["places.sqlite", "default\\places.sqlite"] {
        let p = base.join(rel);
        if p.exists() {
            return Some(json!({ "path": p.to_string_lossy(), "kind": "firefox" }));
        }
    }
    None
}

#[tauri::command]
fn import_from_bookmarks_file(
    state: tauri::State<AppState>,
    path: String,
    name: String,
) -> Result<ImportSummary, String> {
    let filename = std::path::Path::new(&path)
        .file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let (links, folders) = if filename == "places.sqlite" || filename.ends_with(".sqlite") {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::import_firefox(&conn, &path, &name).map_err(|e| e.to_string())?
    } else {
        let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::import_chromium(&conn, &json, &name).map_err(|e| e.to_string())?
    };
    Ok(ImportSummary { links, folders })
}

// ── Browser EXE detection (for Open With / Browser Manager) ─────────────────

#[derive(serde::Serialize)]
struct BrowserExe { name: String, path: String }

fn exe_exists(path: &str) -> bool { std::path::Path::new(path).exists() }

fn reg_query_cmd(key: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", key, "/ve"])
        .output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        for ty in &["REG_SZ", "REG_EXPAND_SZ"] {
            if let Some(idx) = line.find(ty) {
                let val = line[idx + ty.len()..].trim().trim_matches('"');
                // Remove trailing "%1" or " -- %1" etc.
                let val = val.split('"').next().unwrap_or(val)
                    .split(" -- ").next().unwrap_or(val)
                    .trim().trim_matches('"');
                if !val.is_empty() { return Some(val.to_string()); }
            }
        }
    }
    None
}

fn find_versioned_exe(dir: &str, exe_name: &str) -> Option<String> {
    let dir = std::path::Path::new(dir);
    if !dir.is_dir() { return None; }
    let mut candidates: Vec<std::path::PathBuf> = std::fs::read_dir(dir).ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(exe_name))
        .filter(|p| p.exists())
        .collect();
    candidates.sort();
    candidates.pop().map(|p| p.to_string_lossy().into_owned())
}

fn detect_opera_exe(local: &str, pf: &str, pf86: &str) -> Option<String> {
    // 1. Per-user launcher (most common)
    let launcher = format!("{}\\Programs\\Opera\\launcher.exe", local);
    if exe_exists(&launcher) { return Some(launcher); }

    // 2. Versioned per-user subfolder
    if let Some(p) = find_versioned_exe(&format!("{}\\Programs\\Opera", local), "opera.exe") {
        return Some(p);
    }

    // 3. System-wide install
    for base in &[pf, pf86] {
        let l = format!("{}\\Opera\\launcher.exe", base);
        if exe_exists(&l) { return Some(l); }
        let e = format!("{}\\Opera\\opera.exe", base);
        if exe_exists(&e) { return Some(e); }
    }

    // 4. Registry: HKCU / HKLM StartMenuInternet
    for key in &[
        r"HKCU\SOFTWARE\Clients\StartMenuInternet\OperaStable\shell\open\command",
        r"HKLM\SOFTWARE\Clients\StartMenuInternet\OperaStable\shell\open\command",
        r"HKCU\SOFTWARE\Clients\StartMenuInternet\Opera\shell\open\command",
        r"HKLM\SOFTWARE\Clients\StartMenuInternet\Opera\shell\open\command",
    ] {
        if let Some(cmd) = reg_query_cmd(key) {
            let exe = cmd.trim().trim_matches('"');
            if exe_exists(exe) { return Some(exe.to_string()); }
        }
    }

    // 5. Last resort: HKCU Opera Software key
    if let Some(out) = (|| -> Option<String> {
        let o = std::process::Command::new("reg")
            .args(["query", r"HKCU\SOFTWARE\Opera Software", "/v", "Last Install dir"])
            .output().ok()?;
        let s = String::from_utf8_lossy(&o.stdout);
        for line in s.lines() {
            if line.contains("Last Install dir") {
                let dir = line.split_whitespace().last()?;
                let l = format!("{}\\launcher.exe", dir);
                if exe_exists(&l) { return Some(l); }
                let e = format!("{}\\opera.exe", dir);
                if exe_exists(&e) { return Some(e); }
            }
        }
        None
    })() { return Some(out); }

    None
}

fn detect_browser_exes_list() -> Vec<BrowserExe> {
    let local   = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    let pf      = std::env::var("PROGRAMFILES").unwrap_or_default();
    let pf86    = std::env::var("PROGRAMFILES(X86)").unwrap_or_else(|_| pf.clone());

    let mut result = Vec::new();

    let candidates: &[(&str, &[&str])] = &[
        ("Google Chrome", &[
            &format!("{}\\Google\\Chrome\\Application\\chrome.exe", local) as &str,
            &format!("{}\\Google\\Chrome\\Application\\chrome.exe", pf),
            &format!("{}\\Google\\Chrome\\Application\\chrome.exe", pf86),
        ]),
        ("Microsoft Edge", &[
            &format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", pf),
            &format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", pf86),
            &format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", local),
        ]),
        ("Mozilla Firefox", &[
            &format!("{}\\Mozilla Firefox\\firefox.exe", pf),
            &format!("{}\\Mozilla Firefox\\firefox.exe", pf86),
            &format!("{}\\Programs\\Mozilla Firefox\\firefox.exe", local),
        ]),
        ("Brave", &[
            &format!("{}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe", pf),
            &format!("{}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe", local),
        ]),
        ("Vivaldi", &[
            &format!("{}\\Vivaldi\\Application\\vivaldi.exe", local),
            &format!("{}\\Vivaldi\\Application\\vivaldi.exe", pf),
        ]),
        ("Opera GX", &[
            &format!("{}\\Programs\\Opera GX\\launcher.exe", local),
            &format!("{}\\Opera GX Stable\\launcher.exe", pf),
        ]),
        ("Waterfox", &[
            &format!("{}\\Waterfox\\waterfox.exe", pf),
            &format!("{}\\Waterfox\\waterfox.exe", pf86),
        ]),
    ];

    for (name, paths) in candidates {
        let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        if let Some(path) = paths.into_iter().find(|p| exe_exists(p)) {
            result.push(BrowserExe { name: name.to_string(), path });
        }
    }

    // Opera: dedicated multi-path + registry detection
    if let Some(path) = detect_opera_exe(&local, &pf, &pf86) {
        // Insert Opera after Edge if not already added
        if !result.iter().any(|b| b.name == "Opera") {
            result.push(BrowserExe { name: "Opera".to_string(), path });
        }
    }

    // Roaming-based Opera (some system installs)
    let opera_roaming = format!("{}\\Opera Software\\Opera Stable\\opera.exe", roaming);
    if exe_exists(&opera_roaming) && !result.iter().any(|b| b.name == "Opera") {
        result.push(BrowserExe { name: "Opera".to_string(), path: opera_roaming });
    }

    result
}

#[tauri::command]
fn detect_browser_exes() -> Vec<BrowserExe> { detect_browser_exes_list() }

// ── Browser config (portable storage) ───────────────────────────────────────

fn browsers_config_path() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("browsers.json"))
}

#[tauri::command]
fn load_browsers_config() -> String {
    browsers_config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn save_browsers_config(json: String) -> Result<(), String> {
    let path = browsers_config_path().ok_or("Не удалось получить путь")?;
    std::fs::write(path, json.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn detect_browsers() -> Vec<DetectedBrowser> { detect_browsers_list() }

#[tauri::command]
fn import_from_browser(state: tauri::State<AppState>, browser_id: String) -> Result<ImportSummary, String> {
    let browsers = detect_browsers_list();
    let b = browsers.iter().find(|b| b.id == browser_id).ok_or("Браузер не найден")?;
    let kind = b.kind.clone();
    let name = b.name.clone();
    let path = b.bookmarks_path.clone();
    drop(browsers);

    let (links, folders) = if kind == "firefox" {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::import_firefox(&conn, &path, &name).map_err(|e| e.to_string())?
    } else {
        let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::import_chromium(&conn, &json, &name).map_err(|e| e.to_string())?
    };
    Ok(ImportSummary { links, folders })
}

#[tauri::command]
async fn import_txt_lines(state: tauri::State<'_, AppState>, window: tauri::Window) -> Result<usize, String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Импорт URL из TXT (одна строка = одна ссылка)")
        .add_filter("Текстовый файл", &["txt"])
        .pick_file().await.ok_or("Отменено")?;
    let folder_name = file.path().file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Импорт".to_string());
    let content = std::fs::read_to_string(file.path()).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import_txt_urls(&conn, &content, &folder_name).map_err(|e| e.to_string())
}

// ── Import commands ──────────────────────────────────────────────────────────

#[tauri::command]
async fn import_html(state: tauri::State<'_, AppState>, window: tauri::Window) -> Result<usize, String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Импорт закладок из HTML")
        .add_filter("HTML файл", &["html", "htm"])
        .pick_file().await.ok_or("Отменено")?;
    let content = std::fs::read_to_string(file.path()).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import_html(&conn, &content, None).map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_txt(state: tauri::State<'_, AppState>, window: tauri::Window) -> Result<usize, String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Импорт закладок из TXT")
        .add_filter("Текстовый файл", &["txt"])
        .pick_file().await.ok_or("Отменено")?;
    let content = std::fs::read_to_string(file.path()).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import_txt(&conn, &content, None).map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_sync(state: tauri::State<'_, AppState>, window: tauri::Window) -> Result<usize, String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Импорт файла синхронизации")
        .add_filter("Файл синхронизации", &["json"])
        .pick_file().await.ok_or("Отменено")?;
    let content = std::fs::read_to_string(file.path()).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let arr = v["nodes"].as_array().ok_or("Нет массива nodes в файле")?;
    let nodes: Vec<db::RawSyncNode> = arr.iter().map(|n| db::RawSyncNode {
        old_id:     n["id"].as_i64().unwrap_or(0),
        old_parent: n["parent"].as_i64(),
        kind:       n["kind"].as_str().unwrap_or("bookmark").to_string(),
        title:      n["title"].as_str().unwrap_or("").to_string(),
        url:        n["url"].as_str().map(String::from),
        note:       n["note"].as_str().map(String::from),
    }).collect();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import_sync_nodes(&conn, &nodes, None).map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_uadat_pick(state: tauri::State<'_, AppState>, window: tauri::Window) -> Result<usize, String> {
    let file = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Открыть файл данных UA")
        .add_filter("Файл данных", &["dat", "bak"])
        .pick_file().await.ok_or("Отменено")?;
    let path = file.path().to_path_buf();
    let raw = std::fs::read(&path).map_err(|e| e.to_string())?;
    let (text, _, _) = encoding_rs::WINDOWS_1251.decode(&raw);
    let data_dir = path.parent()
        .map(|p| p.join("Data").to_string_lossy().into_owned())
        .unwrap_or_default();
    let nodes = importer::parse(&text);
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import(&conn, &nodes, &data_dir).map_err(|e| e.to_string())
}

// ── Entry point ──────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // Portable mode: store album.db next to the executable so the
            // entire folder can be moved and everything keeps working.
            let exe_dir = std::env::current_exe()?
                .parent()
                .expect("exe has no parent dir")
                .to_path_buf();

            let db_path = exe_dir.join("album.db");
            let conn = Connection::open(&db_path)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            db::init(&conn)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            app.manage(AppState {
                db: Mutex::new(conn),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_tree,
            get_bookmarks,
            is_empty,
            find_uadat,
            import_uadat,
            open_url,
            open_url_with,
            check_url,
            sort_folder,
            sort_all_bookmarks,
            backup_db,
            backup_db_with_data,
            rename_node,
            delete_folder,
            export_folder_html,
            export_folder_txt,
            export_folder_sync,
            clear_thumb,
            refresh_thumb,
            delete_node,
            update_bookmark,
            pick_browser_file,
            update_note,
            search_bookmarks,
            db_stats,
            import_html,
            import_txt,
            import_sync,
            import_uadat_pick,
            detect_browsers,
            import_from_browser,
            import_txt_lines,
            detect_browser_exes,
            load_browsers_config,
            save_browsers_config,
            pick_bookmarks_file,
            pick_profile_folder,
            find_bookmarks_in_folder,
            import_from_bookmarks_file,
            save_text_file,
            set_sort_idx,
            load_settings,
            save_settings,
            load_toolbar_config,
            save_toolbar_config,
            create_folder,
            create_bookmark,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
