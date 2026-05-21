#![windows_subsystem = "windows"]

mod db;
mod net;

use db::Database;
use slint::{Image, ModelRc, SharedString, VecModel};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

slint::include_modules!();

// ── State ────────────────────────────────────────────────────────────────────

struct State {
    db: Database,
    expanded: HashSet<i64>,
    active_folder: Option<i64>,
    selected_bookmark: Option<i64>,  // selected in tree or list
    search_query: String,
    sort_by: SortBy,
    sort_asc: bool,
    data_dir: std::path::PathBuf,
    check_results: std::collections::HashMap<i64, (bool, String)>,
    tree_width: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy { Title, Url }

impl State {
    fn new(db: Database) -> Self {
        let data_dir = std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(std::path::Path::new(".")).join("Data");
        let (tree_width, expanded, active_folder) = State::load_settings();
        State {
            db, expanded, active_folder, selected_bookmark: None,
            search_query: String::new(), sort_by: SortBy::Title, sort_asc: true,
            data_dir, check_results: Default::default(), tree_width,
        }
    }

    fn favicons_dir(&self) -> std::path::PathBuf { self.data_dir.join("favicons") }

    // ── Unified tree model (folders + bookmark leaves) ────────────────────

    fn build_tree_model(&self) -> ModelRc<TreeNode> {
        let all_folders = self.db.get_all_folders().unwrap_or_default();
        let mut children: std::collections::HashMap<Option<i64>, Vec<usize>> = Default::default();
        for (i, f) in all_folders.iter().enumerate() {
            children.entry(f.parent_id).or_default().push(i);
        }
        let favicons = self.db.get_favicons();
        let favicons_dir = self.favicons_dir();
        let mut result = Vec::new();
        self.walk_tree(&all_folders, &children, &favicons, &favicons_dir, None, 0, &mut result);
        ModelRc::new(VecModel::from(result))
    }

    fn walk_tree(&self,
        all: &[db::DbFolder],
        children: &std::collections::HashMap<Option<i64>, Vec<usize>>,
        favicons: &std::collections::HashMap<i64, String>,
        favicons_dir: &std::path::Path,
        parent: Option<i64>, depth: i32,
        out: &mut Vec<TreeNode>)
    {
        if let Some(kids) = children.get(&parent) {
            for &i in kids {
                let f = &all[i];
                let has_ch = children.contains_key(&Some(f.id));
                let count = self.db.bookmark_count(f.id) as i32;
                // Folder is selected if it's the active folder AND no bookmark is selected
                let selected = self.active_folder == Some(f.id) && self.selected_bookmark.is_none();

                out.push(TreeNode {
                    id: f.id as i32, kind: 0,
                    title: SharedString::from(f.title.as_str()),
                    url: SharedString::default(),
                    depth, expanded: self.expanded.contains(&f.id),
                    has_children: has_ch, selected, count,
                    favicon: Image::default(), has_favicon: false,
                });

                if self.expanded.contains(&f.id) {
                    // Recurse subfolders first
                    self.walk_tree(all, children, favicons, favicons_dir, Some(f.id), depth + 1, out);

                    // Then add bookmark leaves
                    if let Ok(bms) = self.db.get_bookmarks(f.id) {
                        for bm in &bms {
                            let (fav_img, has_fav) = load_favicon(bm.id, favicons, favicons_dir);
                            out.push(TreeNode {
                                id: bm.id as i32, kind: 1,
                                title: SharedString::from(bm.title.as_str()),
                                url: SharedString::from(bm.url.as_deref().unwrap_or("")),
                                depth: depth + 1, expanded: false, has_children: false,
                                selected: self.selected_bookmark == Some(bm.id),
                                count: 0, favicon: fav_img, has_favicon: has_fav,
                            });
                        }
                    }
                }
            }
        }
    }

    // ── Right panel: subfolders + bookmarks ──────────────────────────────

    fn build_right_panel_model(&self) -> ModelRc<RightItem> {
        let favicons = self.db.get_favicons();
        let favicons_dir = self.favicons_dir();
        let mut items = Vec::new();

        if let Some(folder_id) = self.active_folder {
            // Subfolders first (папки всегда выше ссылок)
            let all_folders = self.db.get_all_folders().unwrap_or_default();
            for f in all_folders.iter().filter(|f| f.parent_id == Some(folder_id)) {
                items.push(RightItem {
                    id: f.id as i32, kind: 0,
                    title: SharedString::from(f.title.as_str()),
                    url: SharedString::default(),
                    note: SharedString::default(),
                    favicon: Image::default(), has_favicon: false,
                    check_status: SharedString::default(),
                    count: self.db.bookmark_count(f.id) as i32,
                    selected: false,
                });
            }

            // Then bookmarks
            let mut bms = self.db.get_bookmarks(folder_id).unwrap_or_default();
            match self.sort_by {
                SortBy::Title => bms.sort_by(|a, b| {
                    let c = a.title.to_lowercase().cmp(&b.title.to_lowercase());
                    if self.sort_asc { c } else { c.reverse() }
                }),
                SortBy::Url => bms.sort_by(|a, b| {
                    let c = a.url.as_deref().unwrap_or("").to_lowercase()
                        .cmp(&b.url.as_deref().unwrap_or("").to_lowercase());
                    if self.sort_asc { c } else { c.reverse() }
                }),
            }
            for b in &bms {
                let (fav_img, has_fav) = load_favicon(b.id, &favicons, &favicons_dir);
                let check_status = self.check_results.get(&b.id)
                    .map(|(ok, code)| if *ok { "OK".to_string() } else { code.clone() })
                    .unwrap_or_default();
                items.push(RightItem {
                    id: b.id as i32, kind: 1,
                    title: SharedString::from(b.title.as_str()),
                    url: SharedString::from(b.url.as_deref().unwrap_or("")),
                    note: SharedString::from(b.note.as_deref().unwrap_or("")),
                    favicon: fav_img, has_favicon: has_fav,
                    check_status: SharedString::from(check_status.as_str()),
                    count: 0,
                    selected: self.selected_bookmark == Some(b.id),
                });
            }
        }
        ModelRc::new(VecModel::from(items))
    }

    // ── Right panel bookmark list (search / all) ──────────────────────────

    fn build_bookmark_model(&self) -> ModelRc<BookmarkItem> {
        let mut bms = if !self.search_query.is_empty() {
            self.db.search(&self.search_query).unwrap_or_default()
        } else {
            self.active_folder.map(|id| self.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default()
        };

        match self.sort_by {
            SortBy::Title => bms.sort_by(|a, b| {
                let c = a.title.to_lowercase().cmp(&b.title.to_lowercase());
                if self.sort_asc { c } else { c.reverse() }
            }),
            SortBy::Url => bms.sort_by(|a, b| {
                let c = a.url.as_deref().unwrap_or("").to_lowercase()
                    .cmp(&b.url.as_deref().unwrap_or("").to_lowercase());
                if self.sort_asc { c } else { c.reverse() }
            }),
        }

        let favicons = self.db.get_favicons();
        let favicons_dir = self.favicons_dir();
        let vec: Vec<BookmarkItem> = bms.into_iter().map(|b| {
            let (fav_img, has_fav) = load_favicon(b.id, &favicons, &favicons_dir);
            let check_status = self.check_results.get(&b.id)
                .map(|(ok, code)| if *ok { "OK".to_string() } else { code.clone() })
                .unwrap_or_default();
            BookmarkItem {
                id: b.id as i32, title: SharedString::from(b.title.as_str()),
                url: SharedString::from(b.url.as_deref().unwrap_or("")),
                note: SharedString::from(b.note.as_deref().unwrap_or("")),
                favicon: fav_img, has_favicon: has_fav,
                check_status: SharedString::from(check_status.as_str()),
                selected: self.selected_bookmark == Some(b.id),
            }
        }).collect();
        ModelRc::new(VecModel::from(vec))
    }

    fn sort_label(&self) -> SharedString {
        let arrow = if self.sort_asc { "↑" } else { "↓" };
        let by = match self.sort_by { SortBy::Title => "Название", SortBy::Url => "URL" };
        SharedString::from(format!("{by} {arrow}"))
    }

    fn status(&self) -> SharedString {
        if !self.search_query.is_empty() {
            let n = self.db.search(&self.search_query).unwrap_or_default().len();
            return SharedString::from(format!("Поиск: \"{}\"  |  Найдено: {n}", self.search_query));
        }
        let (folders, bms) = self.db.total_counts();
        match self.active_folder {
            Some(id) => {
                let breadcrumb = self.db.get_breadcrumb(id).unwrap_or_default();
                SharedString::from(format!(
                    "{breadcrumb}  |  Ссылок: {}  |  Всего: {folders} папок, {bms} ссылок",
                    self.db.bookmark_count(id)))
            }
            None => SharedString::from(format!("Папок: {folders}  |  Ссылок: {bms}  |  Выберите папку")),
        }
    }

    fn breadcrumb(&self) -> SharedString {
        match self.active_folder {
            Some(id) => SharedString::from(self.db.get_breadcrumb(id).unwrap_or_default().as_str()),
            None => SharedString::default(),
        }
    }

    fn selected_name(&self) -> String {
        if let Some(id) = self.selected_bookmark { return self.db.get_bookmark_title(id).unwrap_or_default(); }
        if let Some(id) = self.active_folder { return self.db.get_folder_title(id).unwrap_or_default(); }
        String::new()
    }

    fn bookmark_ids_ordered(&self) -> Vec<i64> {
        let mut bms = self.active_folder.map(|id| self.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default();
        match self.sort_by {
            SortBy::Title => bms.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            SortBy::Url => bms.sort_by(|a, b| {
                a.url.as_deref().unwrap_or("").to_lowercase().cmp(&b.url.as_deref().unwrap_or("").to_lowercase())
            }),
        }
        if !self.sort_asc { bms.reverse(); }
        bms.iter().map(|b| b.id).collect()
    }

    // ── Settings persistence ──────────────────────────────────────────────

    fn settings_path() -> std::path::PathBuf {
        std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(std::path::Path::new(".")).join("settings.json")
    }

    fn save_settings(&self) {
        let exp: Vec<i64> = self.expanded.iter().copied().collect();
        let exp_str = exp.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        let active = self.active_folder.unwrap_or(0);
        let json = format!("{{\"tree_width\":{},\"expanded\":\"{exp_str}\",\"active_folder\":{active}}}",
            self.tree_width);
        let _ = std::fs::write(Self::settings_path(), json.as_bytes());
    }

    fn load_settings() -> (f32, HashSet<i64>, Option<i64>) {
        let mut width = 240.0f32;
        let mut expanded = HashSet::new();
        let mut active: Option<i64> = None;
        if let Ok(s) = std::fs::read_to_string(Self::settings_path()) {
            if let Some(p) = s.find("\"tree_width\":") {
                let rest = &s[p+13..];
                let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
                if let Ok(v) = rest[..end].parse::<f32>() { width = v.max(100.0).min(500.0); }
            }
            if let Some(p) = s.find("\"expanded\":\"") {
                let rest = &s[p+12..];
                let end = rest.find('"').unwrap_or(rest.len());
                for id_str in rest[..end].split(',') {
                    if let Ok(id) = id_str.parse::<i64>() { expanded.insert(id); }
                }
            }
            if let Some(p) = s.find("\"active_folder\":") {
                let rest = &s[p+16..];
                let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
                if let Ok(id) = rest[..end].parse::<i64>() { if id > 0 { active = Some(id); } }
            }
        }
        (width, expanded, active)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn load_favicon(id: i64, favicons: &std::collections::HashMap<i64, String>, dir: &std::path::Path) -> (Image, bool) {
    if let Some(fname) = favicons.get(&id) {
        if !fname.is_empty() {
            let p = dir.join(fname);
            if p.exists() {
                if let Ok(img) = Image::load_from_path(&p) { return (img, true); }
            }
        }
    }
    (Image::default(), false)
}

fn refresh_ui(ui: &MainWindow, st: &State) {
    ui.set_tree_nodes(st.build_tree_model());
    ui.set_right_items(st.build_right_panel_model());
    ui.set_bookmarks(st.build_bookmark_model());
    ui.set_status_text(st.status());
    ui.set_sort_label(st.sort_label());
    ui.set_breadcrumb(st.breadcrumb());
    update_detail(ui, st);
}

fn update_detail(ui: &MainWindow, st: &State) {
    if let Some(id) = st.selected_bookmark {
        if let Some(bm) = st.db.get_bookmark(id) {
            let favicons = st.db.get_favicons();
            let (fav_img, has_fav) = load_favicon(id, &favicons, &st.favicons_dir());
            ui.set_detail_title(SharedString::from(bm.title.as_str()));
            ui.set_detail_url(SharedString::from(bm.url.as_deref().unwrap_or("")));
            ui.set_detail_note(SharedString::from(bm.note.as_deref().unwrap_or("")));
            ui.set_detail_favicon(fav_img);
            ui.set_detail_has_favicon(has_fav);
            ui.set_active_bookmark(id as i32);
            return;
        }
    }
    ui.set_detail_title(SharedString::default());
    ui.set_detail_url(SharedString::default());
    ui.set_detail_note(SharedString::default());
    ui.set_detail_favicon(Image::default());
    ui.set_detail_has_favicon(false);
    ui.set_active_bookmark(0);
}

fn open_url(url: &str) {
    let url = normalize_url(url);
    if url.is_empty() { return; }
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url.as_str()])
        .creation_flags(0x0800_0000).spawn();
}

fn normalize_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() { return String::new(); }
    if url.contains("://") || url.starts_with("mailto:") { return url.to_string(); }
    format!("https://{url}")
}

fn copy_to_clipboard(text: &str) {
    extern "system" {
        fn OpenClipboard(hwnd: *mut core::ffi::c_void) -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(fmt: u32, hmem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn CloseClipboard() -> i32;
        fn GlobalAlloc(flags: u32, size: usize) -> *mut core::ffi::c_void;
        fn GlobalLock(hmem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn GlobalUnlock(hmem: *mut core::ffi::c_void) -> i32;
    }
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 { return; }
        EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let hmem = GlobalAlloc(0x0042, wide.len() * 2);
        if !hmem.is_null() {
            let ptr = GlobalLock(hmem) as *mut u16;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                GlobalUnlock(hmem);
                SetClipboardData(13, hmem);
            }
        }
        CloseClipboard();
    }
}

fn show_ctx_folder(ui: &MainWindow, id: i32) {
    ui.set_ctx_is_folder(true); ui.set_ctx_id(id); ui.set_show_ctx(true);
}

fn show_ctx_bookmark(ui: &MainWindow, st: &State, id: i32) {
    ui.set_ctx_is_folder(false); ui.set_ctx_id(id); ui.set_show_ctx(true);
    if let Some(bm) = st.db.get_bookmark(id as i64) {
        let favicons = st.db.get_favicons();
        let (fav_img, has_fav) = load_favicon(id as i64, &favicons, &st.favicons_dir());
        ui.set_detail_title(SharedString::from(bm.title.as_str()));
        ui.set_detail_url(SharedString::from(bm.url.as_deref().unwrap_or("")));
        ui.set_detail_note(SharedString::from(bm.note.as_deref().unwrap_or("")));
        ui.set_detail_favicon(fav_img);
        ui.set_detail_has_favicon(has_fav);
    }
}

fn last_db_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default()
        .parent().unwrap_or(std::path::Path::new(".")).join("last_db.txt")
}

fn save_last_db(path: &std::path::Path) {
    let _ = std::fs::write(last_db_path(), path.to_string_lossy().as_bytes());
}

fn browser_bookmark_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    for p in [
        format!("{local}\\Google\\Chrome\\User Data\\Default\\Bookmarks"),
        format!("{local}\\Microsoft\\Edge\\User Data\\Default\\Bookmarks"),
        format!("{local}\\BraveSoftware\\Brave-Browser\\User Data\\Default\\Bookmarks"),
        format!("{roaming}\\Opera Software\\Opera Stable\\Bookmarks"),
    ] { let pb = std::path::PathBuf::from(p); if pb.exists() { paths.push(pb); } }
    paths
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let db_path = std::fs::read_to_string(last_db_path()).ok()
        .map(|s| std::path::PathBuf::from(s.trim()))
        .filter(|p| p.exists())
        .unwrap_or_else(Database::default_path);

    let db = Database::open_at(&db_path).unwrap_or_else(|_| Database::open_default().expect("DB"));
    db.init_schema().expect("Schema");
    save_last_db(&db_path);

    let state = Arc::new(Mutex::new(State::new(db)));
    let ui = MainWindow::new().unwrap();

    {
        let st = state.lock().unwrap();
        ui.set_tree_width_px(st.tree_width as i32);
        let db_name = db_path.file_name().unwrap_or_default().to_string_lossy().to_string();
        ui.set_db_name(SharedString::from(db_name.as_str()));
    }
    refresh_ui(&ui, &state.lock().unwrap());

    // ── Tree navigation ───────────────────────────────────────────────────

    // Folder clicked → select + show contents (WITHOUT changing expanded state)
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_tree_folder_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        st.save_settings();  // do NOT auto-expand - that's [+]/[-] button's job
        refresh_ui(&ui, &st); }); }

    // Folder toggled (double-click or [+]/[-])
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_tree_folder_toggled(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let fid = id as i64;
        if st.expanded.contains(&fid) { st.expanded.remove(&fid); } else { st.expanded.insert(fid); }
        st.save_settings();
        ui.set_tree_nodes(st.build_tree_model()); }); }

    // Bookmark clicked in tree → show detail card
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_tree_bookmark_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        ui.set_tree_nodes(st.build_tree_model());
        update_detail(&ui, &st); }); }

    // Back to list mode (Escape or "← Назад")
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_back_to_list(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = None;
        ui.set_active_bookmark(0);
        ui.set_tree_nodes(st.build_tree_model());
        ui.set_bookmarks(st.build_bookmark_model()); }); }

    // Open URL
    ui.on_bookmark_open(|url| open_url(url.as_str()));

    // Arrow key nav in list mode
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_bookmark_nav(move |delta| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let ids = st.bookmark_ids_ordered();
        if ids.is_empty() { return; }
        let cur = st.selected_bookmark.and_then(|sel| ids.iter().position(|&id| id == sel));
        let new_pos = match cur { None => 0, Some(p) => ((p as i32 + delta).rem_euclid(ids.len() as i32)) as usize };
        st.selected_bookmark = Some(ids[new_pos]);
        ui.set_bookmarks(st.build_bookmark_model()); update_detail(&ui, &st); }); }

    // Navigate into subfolder from right panel
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_right_folder_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        st.expanded.insert(id as i64); st.save_settings();
        refresh_ui(&ui, &st); }); }

    // Bookmark clicked in right panel → show detail
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_right_bookmark_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        ui.set_right_items(st.build_right_panel_model());
        update_detail(&ui, &st); }); }

    // ── CRUD ─────────────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_folder_confirmed(move |name| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let parent = st.active_folder;
        if let Ok(new_id) = st.db.create_folder(parent, name.as_str()) {
            if let Some(p) = parent { st.expanded.insert(p); }
            st.active_folder = Some(new_id); st.expanded.insert(new_id); st.save_settings();
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_bookmark_confirmed(move |title, url, note| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        // Check for duplicate URL
        let url_str = url.as_str();
        if !url_str.is_empty() {
            if let Ok(existing) = st.db.search(url_str) {
                if existing.iter().any(|b| b.url.as_deref() == Some(url_str)) {
                    ui.set_status_text(SharedString::from(
                        format!("Дублирующийся URL: {url_str}")));
                }
            }
        }
        let fid = st.active_folder.unwrap_or_else(|| st.db.create_folder(None, "Ссылки").unwrap_or(1));
        st.active_folder = Some(fid);
        if let Ok(new_id) = st.db.create_bookmark(fid, title.as_str(), url_str) {
            // Save note if provided
            if !note.as_str().is_empty() {
                let _ = st.db.update_bookmark(new_id, title.as_str(), url_str, note.as_str());
            }
            st.selected_bookmark = Some(new_id);
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_rename_requested(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        ui.set_rename_prefill(SharedString::from(st.selected_name().as_str()));
        ui.set_show_rename_dlg(true); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_rename_confirmed(move |new_name| {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(id) = st.selected_bookmark { let _ = st.db.rename_node(id, new_name.as_str()); }
        else if let Some(id) = st.active_folder { let _ = st.db.rename_node(id, new_name.as_str()); }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_edit_requested(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(id) = st.selected_bookmark {
            if let Some(bm) = st.db.get_bookmark(id) {
                ui.set_edit_title_val(SharedString::from(bm.title.as_str()));
                ui.set_edit_url_val(SharedString::from(bm.url.as_deref().unwrap_or("")));
                ui.set_edit_note_val(SharedString::from(bm.note.as_deref().unwrap_or("")));
                ui.set_show_edit_dlg(true);
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_edit_confirmed(move |title, url, note| {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(id) = st.selected_bookmark {
            let _ = st.db.update_bookmark(id, title.as_str(), url.as_str(), note.as_str());
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_delete_selected(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        // Show confirm dialog
        let text = if let Some(id) = st.selected_bookmark {
            st.db.get_bookmark_title(id).map(|t| format!("Удалить ссылку «{t}»?"))
                .unwrap_or_else(|| "Удалить ссылку?".to_string())
        } else if let Some(id) = st.active_folder {
            st.db.get_folder_title(id).map(|t| format!("Удалить папку «{t}» и всё содержимое?"))
                .unwrap_or_else(|| "Удалить папку?".to_string())
        } else { return; };
        ui.set_confirm_delete_text(SharedString::from(text.as_str()));
        ui.set_show_confirm_delete(true); }); }

    // Actually perform delete after confirmation
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_confirm_delete_yes(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        if let Some(id) = st.selected_bookmark {
            let _ = st.db.delete_bookmark(id); st.selected_bookmark = None;
            ui.set_active_bookmark(0);
        } else if let Some(id) = st.active_folder {
            let _ = st.db.delete_folder(id); st.expanded.remove(&id); st.active_folder = None;
            st.save_settings();
        }
        refresh_ui(&ui, &st); }); }

    // ── Context menu ──────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_folder_right_click(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        ui.set_tree_nodes(st.build_tree_model()); show_ctx_folder(&ui, id); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_bookmark_right_click(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        ui.set_tree_nodes(st.build_tree_model()); show_ctx_bookmark(&ui, &st, id); }); }

    { let s = state.clone();
      ui.on_ctx_open(move |id| {
        let st = s.lock().unwrap();
        if let Some(bm) = st.db.get_bookmark(id as i64) { open_url(bm.url.as_deref().unwrap_or("")); } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_edit(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        if let Some(bm) = st.db.get_bookmark(id as i64) {
            ui.set_edit_title_val(SharedString::from(bm.title.as_str()));
            ui.set_edit_url_val(SharedString::from(bm.url.as_deref().unwrap_or("")));
            ui.set_edit_note_val(SharedString::from(bm.note.as_deref().unwrap_or("")));
            ui.set_show_edit_dlg(true);
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_rename(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let is_folder = ui.get_ctx_is_folder();
        let name = if is_folder { st.db.get_folder_title(id as i64) } else { st.db.get_bookmark_title(id as i64) }.unwrap_or_default();
        if is_folder { st.active_folder = Some(id as i64); st.selected_bookmark = None; }
        else { st.selected_bookmark = Some(id as i64); }
        ui.set_rename_prefill(SharedString::from(name.as_str())); ui.set_show_rename_dlg(true); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_delete(move |id| {
        let ui = w.unwrap(); let is_folder = ui.get_ctx_is_folder();
        let mut st = s.lock().unwrap();
        if is_folder { let _ = st.db.delete_folder(id as i64); st.expanded.remove(&(id as i64)); if st.active_folder == Some(id as i64) { st.active_folder = None; } st.save_settings(); }
        else { let _ = st.db.delete_bookmark(id as i64); if st.selected_bookmark == Some(id as i64) { st.selected_bookmark = None; ui.set_active_bookmark(0); } }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_new_sub(move |pid| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        if let Ok(new_id) = st.db.create_folder(Some(pid as i64), "Новая папка") {
            st.expanded.insert(pid as i64); st.active_folder = Some(new_id); st.expanded.insert(new_id); st.save_settings();
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_new_bm_in(move |pid| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(pid as i64); st.expanded.insert(pid as i64); st.save_settings();
        refresh_ui(&ui, &st); ui.set_show_bookmark_dlg(true); }); }

    { let s = state.clone();
      ui.on_ctx_copy_url(move |id| {
        let st = s.lock().unwrap();
        if let Some(bm) = st.db.get_bookmark(id as i64) { copy_to_clipboard(bm.url.as_deref().unwrap_or("")); } }); }

    { let w = ui.as_weak();
      ui.on_ctx_move(move |id| {
        let ui = w.unwrap(); ui.set_ctx_id(id); ui.set_show_move_dlg(true); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_move_confirm(move |target_folder_id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let bm_id = ui.get_ctx_id() as i64;
        let _ = st.db.move_node(bm_id, target_folder_id as i64);
        st.selected_bookmark = None; ui.set_active_bookmark(0);
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_export_folder(move |folder_id| {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("HTML", &["html"]).set_file_name("folder.html").save_file() {
            let bms = st.db.get_bookmarks(folder_id as i64).unwrap_or_default();
            let mut html = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n");
            let mut count = 0;
            for b in &bms { if let Some(url) = &b.url { html.push_str(&format!("    <DT><A HREF=\"{url}\">{}</A>\n", b.title)); count += 1; } }
            html.push_str("</DL><p>\n");
            match std::fs::write(&path, html.as_bytes()) {
                Ok(_) => ui.set_status_text(SharedString::from(format!("Экспорт: {count} ссылок"))),
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_load_favicons_folder(move |folder_id| {
        let ui = w.unwrap();
        let bms = { let st = s.lock().unwrap(); st.db.get_bookmarks(folder_id as i64).unwrap_or_default() };
        let favicons_dir = { s.lock().unwrap().favicons_dir() };
        let total = bms.len(); if total == 0 { return; }
        ui.set_show_favicon_progress(true);
        ui.set_favicon_progress_text(SharedString::from(format!("Favicon: 0 / {total}")));
        ui.set_favicon_progress_value(0.0);
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            for (i, bm) in bms.into_iter().enumerate() {
                if let Some(url) = &bm.url {
                    if let Some(fname) = net::fetch_favicon(url, &favicons_dir) {
                        let _ = s2.lock().unwrap().db.set_favicon(bm.id, &fname);
                    }
                }
                let done = i + 1;
                let progress = done as f32 / total as f32;
                let s3 = s2.clone(); let w3 = w2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w3.unwrap();
                    ui.set_favicon_progress_text(SharedString::from(format!("Favicon: {done} / {total}")));
                    ui.set_favicon_progress_value(progress);
                    if done == total {
                        let st = s3.lock().unwrap();
                        ui.set_tree_nodes(st.build_tree_model());
                        ui.set_right_items(st.build_right_panel_model());
                        ui.set_bookmarks(st.build_bookmark_model());
                        ui.set_status_text(st.status());
                        ui.set_show_favicon_progress(false);
                    }
                });
            }
        });
    }); }

    // ── Sort / Search ─────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_sort_toggle(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        match (st.sort_by, st.sort_asc) {
            (SortBy::Title, true) => st.sort_asc = false,
            (SortBy::Title, false) => { st.sort_by = SortBy::Url; st.sort_asc = true; }
            (SortBy::Url, true) => st.sort_asc = false,
            (SortBy::Url, false) => { st.sort_by = SortBy::Title; st.sort_asc = true; }
        }
        ui.set_sort_label(st.sort_label()); ui.set_bookmarks(st.build_bookmark_model()); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_search_changed(move |query| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.search_query = query.to_string(); st.selected_bookmark = None;
        ui.set_active_bookmark(0); ui.set_bookmarks(st.build_bookmark_model()); ui.set_status_text(st.status()); }); }

    // ── DB operations ─────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_open_db(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Database", &["db"]).add_filter("All", &["*"]).pick_file() {
            match Database::open_at(&path) {
                Ok(new_db) => { let _ = new_db.init_schema(); let mut st = s.lock().unwrap();
                    st.db = new_db; st.expanded.clear(); st.active_folder = None; st.selected_bookmark = None; st.search_query.clear(); st.check_results.clear(); st.save_settings();
                    save_last_db(&path);
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    ui.set_db_name(SharedString::from(name.as_str())); ui.set_active_bookmark(0);
                    refresh_ui(&ui, &st); }
                Err(e) => { ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))); }
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_db(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Database", &["db"]).set_file_name("album.db").save_file() {
            let _ = std::fs::remove_file(&path);
            match Database::open_at(&path) {
                Ok(new_db) => { let _ = new_db.init_schema(); let mut st = s.lock().unwrap();
                    st.db = new_db; st.expanded.clear(); st.active_folder = None; st.selected_bookmark = None; st.search_query.clear(); st.check_results.clear(); st.save_settings();
                    save_last_db(&path);
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    ui.set_db_name(SharedString::from(name.as_str())); ui.set_active_bookmark(0);
                    refresh_ui(&ui, &st); }
                Err(e) => { ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))); }
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_backup_db(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Database", &["db"]).set_file_name("backup.db").save_file() {
            match st.db.backup(&path) {
                Ok(_) => ui.set_status_text(SharedString::from(format!("Копия: {}", path.display()))),
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    // ── Import / Export ───────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_export_html(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("HTML", &["html"]).set_file_name("bookmarks.html").save_file() {
            match st.db.export_html(&path) {
                Ok(n) => ui.set_status_text(SharedString::from(format!("Экспорт HTML: {n} ссылок"))),
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_export_txt(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Text", &["txt"]).set_file_name("bookmarks.txt").save_file() {
            match st.db.export_txt(&path) {
                Ok(n) => ui.set_status_text(SharedString::from(format!("Экспорт TXT: {n} ссылок"))),
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_uadat(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("ua.dat", &["dat"]).add_filter("All", &["*"]).set_file_name("ua.dat").pick_file() {
            let mut st = s.lock().unwrap();
            match st.db.import_uadat(&path) {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт ua.dat: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_html(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("HTML", &["html","htm"]).add_filter("All", &["*"]).pick_file() {
            let mut st = s.lock().unwrap();
            match st.db.import_html(&path) {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт HTML: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_browser(move || {
        let ui = w.unwrap();
        let candidates = browser_bookmark_paths();
        let mut dialog = rfd::FileDialog::new().add_filter("Bookmarks JSON", &["json", "Bookmarks"]).add_filter("All files", &["*"]);
        if let Some(first) = candidates.first() { if let Some(dir) = first.parent() { dialog = dialog.set_directory(dir); } }
        if let Some(path) = dialog.pick_file() {
            let mut st = s.lock().unwrap();
            match st.db.import_chrome_json(&path) {
                Ok(n) if n > 0 => { ui.set_status_text(SharedString::from(format!("Импорт браузера: {n} ссылок"))); refresh_ui(&ui, &st); }
                _ => match st.db.import_html(&path) {
                    Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт HTML: {n} ссылок"))); refresh_ui(&ui, &st); }
                    Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
                }
            }
        } }); }

    // ── Favicon loading ───────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_load_favicons(move || {
        let ui = w.unwrap();
        let (bms, favicons_dir) = { let st = s.lock().unwrap();
            (st.active_folder.map(|id| st.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default(), st.favicons_dir()) };
        let total = bms.len();
        if total == 0 { ui.set_status_text(SharedString::from("Нет ссылок для загрузки favicon")); return; }
        ui.set_show_favicon_progress(true);
        ui.set_favicon_progress_text(SharedString::from(format!("Favicon: 0 / {total}")));
        ui.set_favicon_progress_value(0.0);
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            for (i, bm) in bms.into_iter().enumerate() {
                if let Some(url) = &bm.url {
                    if let Some(fname) = net::fetch_favicon(url, &favicons_dir) {
                        let _ = s2.lock().unwrap().db.set_favicon(bm.id, &fname);
                    }
                }
                let done = i + 1;
                let progress = done as f32 / total as f32;
                let s3 = s2.clone(); let w3 = w2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w3.unwrap();
                    ui.set_favicon_progress_text(SharedString::from(format!("Favicon: {done} / {total}")));
                    ui.set_favicon_progress_value(progress);
                    if done == total {
                        let st = s3.lock().unwrap();
                        ui.set_tree_nodes(st.build_tree_model());
                        ui.set_right_items(st.build_right_panel_model());
                        ui.set_bookmarks(st.build_bookmark_model());
                        ui.set_status_text(st.status());
                        ui.set_show_favicon_progress(false);
                    }
                });
            }
        });
    }); }

    // ── Check links ───────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_check_links(move || {
        let ui = w.unwrap();
        let bms = { let st = s.lock().unwrap(); st.active_folder.map(|id| st.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default() };
        let total = bms.len(); if total == 0 { ui.set_status_text(SharedString::from("Нет ссылок")); return; }
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            for (i, bm) in bms.into_iter().enumerate() {
                let result = bm.url.as_deref().map(net::check_url).unwrap_or((false, "no url".to_string()));
                let done = i + 1; let s3 = s2.clone(); let w3 = w2.clone(); let bm_id = bm.id;
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w3.unwrap(); s3.lock().unwrap().check_results.insert(bm_id, result);
                    if done == total { let st = s3.lock().unwrap(); ui.set_bookmarks(st.build_bookmark_model()); ui.set_status_text(st.status()); }
                    else { ui.set_status_text(SharedString::from(format!("Проверка: {done}/{total}..."))); }
                });
            }
        });
        ui.set_status_text(SharedString::from(format!("Проверяю {total} ссылок...")));
    }); }

    // ── Misc ──────────────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_find_duplicates(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let dups = st.db.find_duplicates().unwrap_or_default(); let n = dups.len();
        if n == 0 { ui.set_status_text(SharedString::from("Дубликатов не найдено")); return; }
        st.active_folder = None; st.selected_bookmark = None;
        let favicons = st.db.get_favicons(); let favicons_dir = st.favicons_dir();
        let vec: Vec<BookmarkItem> = dups.into_iter().map(|b| {
            let (fav_img, has_fav) = load_favicon(b.id, &favicons, &favicons_dir);
            BookmarkItem { id: b.id as i32, title: SharedString::from(b.title.as_str()),
                url: SharedString::from(b.url.as_deref().unwrap_or("")),
                note: SharedString::from(b.note.as_deref().unwrap_or("")),
                favicon: fav_img, has_favicon: has_fav, check_status: SharedString::default(), selected: false }
        }).collect();
        ui.set_active_bookmark(0); ui.set_bookmarks(ModelRc::new(VecModel::from(vec)));
        ui.set_tree_nodes(st.build_tree_model());
        ui.set_status_text(SharedString::from(format!("Дубликатов: {n} — удалите лишние (Del)"))); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_show_all(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let all = st.db.get_all_bookmarks().unwrap_or_default(); let n = all.len();
        st.active_folder = None; st.selected_bookmark = None;
        let favicons = st.db.get_favicons(); let favicons_dir = st.favicons_dir();
        let vec: Vec<BookmarkItem> = all.into_iter().map(|b| {
            let (fav_img, has_fav) = load_favicon(b.id, &favicons, &favicons_dir);
            BookmarkItem { id: b.id as i32, title: SharedString::from(b.title.as_str()),
                url: SharedString::from(b.url.as_deref().unwrap_or("")),
                note: SharedString::from(b.note.as_deref().unwrap_or("")),
                favicon: fav_img, has_favicon: has_fav, check_status: SharedString::default(), selected: false }
        }).collect();
        ui.set_active_bookmark(0); ui.set_bookmarks(ModelRc::new(VecModel::from(vec)));
        ui.set_tree_nodes(st.build_tree_model());
        ui.set_status_text(SharedString::from(format!("Все ссылки: {n}"))); }); }

    { let s = state.clone();
      ui.on_tree_width_changed(move |w| {
        let mut st = s.lock().unwrap(); st.tree_width = w as f32; st.save_settings(); }); }

    // Expand all folders
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_expand_all(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let all = st.db.get_all_folders().unwrap_or_default();
        for f in &all { st.expanded.insert(f.id); }
        st.save_settings(); ui.set_tree_nodes(st.build_tree_model()); }); }

    // Collapse all folders
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_collapse_all(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.expanded.clear(); st.save_settings();
        ui.set_tree_nodes(st.build_tree_model()); }); }

    ui.on_focus_search(|| {});

    ui.run().unwrap();
}
