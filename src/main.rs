mod db;
mod net;

use db::Database;
use slint::{Image, ModelRc, SharedString, VecModel};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

slint::include_modules!();

struct State {
    db: Database,
    expanded: HashSet<i64>,
    active_folder: Option<i64>,
    selected_bookmark: Option<i64>,
    search_query: String,
    sort_by: SortBy,
    sort_asc: bool,
    data_dir: std::path::PathBuf,
    check_results: std::collections::HashMap<i64, (bool, String)>,
    tree_width: f32,
}

impl State {
    fn settings_path() -> std::path::PathBuf {
        std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(std::path::Path::new(".")).join("settings.json")
    }
    fn save_settings(&self) {
        let json = format!("{{\"tree_width\":{}}}", self.tree_width);
        let _ = std::fs::write(Self::settings_path(), json.as_bytes());
    }
    fn load_settings() -> f32 {
        let path = Self::settings_path();
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Some(start) = s.find("\"tree_width\":") {
                let rest = &s[start + 13..];
                let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
                if let Ok(v) = rest[..end].parse::<f32>() { return v.max(100.0).min(500.0); }
            }
        }
        210.0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy { Title, Url }

impl State {
    fn new(db: Database) -> Self {
        let data_dir = std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(std::path::Path::new(".")).join("Data");
        let tree_width = State::load_settings();
        State {
            db, expanded: HashSet::new(),
            active_folder: None, selected_bookmark: None,
            search_query: String::new(),
            sort_by: SortBy::Title, sort_asc: true,
            data_dir, check_results: Default::default(),
            tree_width,
        }
    }

    fn favicons_dir(&self) -> std::path::PathBuf { self.data_dir.join("favicons") }

    fn build_folder_model(&self) -> ModelRc<FolderNode> {
        let all = self.db.get_all_folders().unwrap_or_default();
        let mut children: std::collections::HashMap<Option<i64>, Vec<usize>> = Default::default();
        for (i, f) in all.iter().enumerate() { children.entry(f.parent_id).or_default().push(i); }
        let mut result: Vec<FolderNode> = Vec::new();
        Self::walk(&all, &children, &self.expanded, self.active_folder, None, 0, &mut result);
        ModelRc::new(VecModel::from(result))
    }

    fn walk(all: &[db::DbFolder], children: &std::collections::HashMap<Option<i64>, Vec<usize>>,
            expanded: &HashSet<i64>, active: Option<i64>, parent: Option<i64>, depth: i32,
            out: &mut Vec<FolderNode>) {
        if let Some(kids) = children.get(&parent) {
            for &i in kids {
                let f = &all[i];
                out.push(FolderNode {
                    id: f.id as i32, title: SharedString::from(f.title.as_str()),
                    depth, expanded: expanded.contains(&f.id),
                    has_children: children.contains_key(&Some(f.id)),
                    selected: active == Some(f.id),
                });
                if expanded.contains(&f.id) {
                    Self::walk(all, children, expanded, active, Some(f.id), depth + 1, out);
                }
            }
        }
    }

    fn build_bookmark_model(&self) -> ModelRc<BookmarkItem> {
        let mut bms = if !self.search_query.is_empty() {
            self.db.search(&self.search_query).unwrap_or_default()
        } else {
            self.active_folder.map(|id| self.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default()
        };
        match self.sort_by {
            SortBy::Title => bms.sort_by(|a, b| {
                let cmp = a.title.to_lowercase().cmp(&b.title.to_lowercase());
                if self.sort_asc { cmp } else { cmp.reverse() }
            }),
            SortBy::Url => bms.sort_by(|a, b| {
                let cmp = a.url.as_deref().unwrap_or("").to_lowercase()
                    .cmp(&b.url.as_deref().unwrap_or("").to_lowercase());
                if self.sort_asc { cmp } else { cmp.reverse() }
            }),
        }
        let favicons = self.db.get_favicons();
        let favicons_dir = self.favicons_dir();
        let vec: Vec<BookmarkItem> = bms.into_iter().map(|b| {
            let favicon_file = favicons.get(&b.id).cloned().unwrap_or_default();
            let (favicon_img, has_favicon) = if !favicon_file.is_empty() {
                let path = favicons_dir.join(&favicon_file);
                if path.exists() {
                    match Image::load_from_path(&path) {
                        Ok(img) => (img, true),
                        Err(_) => (Image::default(), false),
                    }
                } else { (Image::default(), false) }
            } else { (Image::default(), false) };

            let check_status = self.check_results.get(&b.id)
                .map(|(ok, code)| if *ok { "OK".to_string() } else { code.clone() })
                .unwrap_or_default();

            BookmarkItem {
                id: b.id as i32, title: SharedString::from(b.title.as_str()),
                url: SharedString::from(b.url.as_deref().unwrap_or("")),
                note: SharedString::from(b.note.as_deref().unwrap_or("")),
                favicon: favicon_img, has_favicon,
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
            Some(id) => SharedString::from(format!(
                "Ссылок: {}  |  Всего: папок {folders}, ссылок {bms}  |  ↑↓ Enter Del F2 F4",
                self.db.bookmark_count(id))),
            None => SharedString::from(format!("Папок: {folders}  |  Ссылок: {bms}  |  Выберите папку")),
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
}

fn refresh_ui(ui: &MainWindow, st: &State) {
    ui.set_folders(st.build_folder_model());
    ui.set_bookmarks(st.build_bookmark_model());
    ui.set_status_text(st.status());
    ui.set_sort_label(st.sort_label());
    update_detail(ui, st);
}

fn update_detail(ui: &MainWindow, st: &State) {
    if let Some(id) = st.selected_bookmark {
        if let Some(bm) = st.db.get_bookmark(id) {
            ui.set_detail_title(SharedString::from(bm.title.as_str()));
            ui.set_detail_url(SharedString::from(bm.url.as_deref().unwrap_or("")));
            ui.set_detail_note(SharedString::from(bm.note.as_deref().unwrap_or("")));
            return;
        }
    }
    ui.set_detail_title(SharedString::default());
    ui.set_detail_url(SharedString::default());
    ui.set_detail_note(SharedString::default());
}

fn open_url(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000).spawn();
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
        ui.set_detail_title(SharedString::from(bm.title.as_str()));
        ui.set_detail_url(SharedString::from(bm.url.as_deref().unwrap_or("")));
        ui.set_detail_note(SharedString::from(bm.note.as_deref().unwrap_or("")));
    }
}

fn main() {
    let db = Database::open_default().expect("Cannot open database");
    db.init_schema().expect("Cannot init schema");

    let state = Arc::new(Mutex::new(State::new(db)));
    let ui = MainWindow::new().unwrap();
    { let st = state.lock().unwrap();
      ui.set_tree_width_px(st.tree_width as i32); }
    refresh_ui(&ui, &state.lock().unwrap());

    // ── Splitter ──────────────────────────────────────────────────────────────
    { let s = state.clone();
      ui.on_tree_width_changed(move |w| {
        let mut st = s.lock().unwrap();
        st.tree_width = w as f32;
        st.save_settings(); }); }

    // ── Navigation ────────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_folder_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        st.expanded.insert(id as i64); refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_folder_toggle(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let fid = id as i64;
        if st.expanded.contains(&fid) { st.expanded.remove(&fid); } else { st.expanded.insert(fid); }
        ui.set_folders(st.build_folder_model()); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_bookmark_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        ui.set_bookmarks(st.build_bookmark_model()); update_detail(&ui, &st); }); }

    ui.on_bookmark_open(|url| open_url(url.as_str()));

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_bookmark_nav(move |delta| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let ids = st.bookmark_ids_ordered();
        if ids.is_empty() { return; }
        let cur = st.selected_bookmark.and_then(|sel| ids.iter().position(|&id| id == sel));
        let new_pos = match cur { None => 0, Some(p) => ((p as i32 + delta).rem_euclid(ids.len() as i32)) as usize };
        st.selected_bookmark = Some(ids[new_pos]);
        ui.set_bookmarks(st.build_bookmark_model()); update_detail(&ui, &st); }); }

    // ── CRUD ─────────────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_folder_confirmed(move |name| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let parent = st.active_folder;
        if let Ok(new_id) = st.db.create_folder(parent, name.as_str()) {
            if let Some(p) = parent { st.expanded.insert(p); }
            st.active_folder = Some(new_id); st.expanded.insert(new_id);
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_bookmark_confirmed(move |title, url| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let fid = st.active_folder.unwrap_or_else(|| st.db.create_folder(None, "Ссылки").unwrap_or(1));
        st.active_folder = Some(fid);
        let _ = st.db.create_bookmark(fid, title.as_str(), url.as_str());
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
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        if let Some(id) = st.selected_bookmark {
            let _ = st.db.delete_bookmark(id); st.selected_bookmark = None;
        } else if let Some(id) = st.active_folder {
            let _ = st.db.delete_folder(id); st.expanded.remove(&id); st.active_folder = None;
        }
        refresh_ui(&ui, &st); }); }

    // ── Context menu ─────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_folder_right_click(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        ui.set_folders(st.build_folder_model()); show_ctx_folder(&ui, id); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_bookmark_right_click(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64); ui.set_bookmarks(st.build_bookmark_model());
        show_ctx_bookmark(&ui, &st, id); }); }

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
        ui.set_rename_prefill(SharedString::from(name.as_str()));
        ui.set_show_rename_dlg(true); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_delete(move |id| {
        let ui = w.unwrap(); let is_folder = ui.get_ctx_is_folder();
        let mut st = s.lock().unwrap();
        if is_folder {
            let _ = st.db.delete_folder(id as i64); st.expanded.remove(&(id as i64));
            if st.active_folder == Some(id as i64) { st.active_folder = None; }
        } else {
            let _ = st.db.delete_bookmark(id as i64);
            if st.selected_bookmark == Some(id as i64) { st.selected_bookmark = None; }
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_new_sub(move |parent_id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        if let Ok(new_id) = st.db.create_folder(Some(parent_id as i64), "Новая папка") {
            st.expanded.insert(parent_id as i64); st.active_folder = Some(new_id); st.expanded.insert(new_id);
        }
        refresh_ui(&ui, &st); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_new_bm_in(move |parent_id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(parent_id as i64); st.expanded.insert(parent_id as i64);
        refresh_ui(&ui, &st); ui.set_show_bookmark_dlg(true); }); }

    { let s = state.clone();
      ui.on_ctx_copy_url(move |id| {
        let st = s.lock().unwrap();
        if let Some(bm) = st.db.get_bookmark(id as i64) { copy_to_clipboard(bm.url.as_deref().unwrap_or("")); } }); }

    // ── Sort ─────────────────────────────────────────────────────────────────

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

    // ── Search ────────────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_search_changed(move |query| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.search_query = query.to_string(); st.selected_bookmark = None;
        ui.set_detail_url(SharedString::default());
        ui.set_bookmarks(st.build_bookmark_model()); ui.set_status_text(st.status()); }); }

    // ── DB operations ─────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_open_db(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Database", &["db"]).add_filter("All", &["*"]).pick_file() {
            match Database::open_at(&path) {
                Ok(new_db) => { let _ = new_db.init_schema();
                    let mut st = s.lock().unwrap(); st.db = new_db; st.expanded.clear();
                    st.active_folder = None; st.selected_bookmark = None; st.search_query.clear(); st.check_results.clear();
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    ui.set_status_text(SharedString::from(format!("Открыта: {name}")));
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
                Ok(new_db) => { let _ = new_db.init_schema();
                    let mut st = s.lock().unwrap(); st.db = new_db; st.expanded.clear();
                    st.active_folder = None; st.selected_bookmark = None; st.search_query.clear(); st.check_results.clear();
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    ui.set_status_text(SharedString::from(format!("Создана: {name}")));
                    refresh_ui(&ui, &st); }
                Err(e) => { ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))); }
            }
        } }); }

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

    // ── Favicon loading ───────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_load_favicons(move || {
        let ui = w.unwrap();
        let (bms, favicons_dir) = {
            let st = s.lock().unwrap();
            let bms = st.active_folder.map(|id| st.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default();
            (bms, st.favicons_dir())
        };
        let total = bms.len();
        if total == 0 { ui.set_status_text(SharedString::from("Нет ссылок")); return; }
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            for (i, bm) in bms.into_iter().enumerate() {
                if let Some(url) = &bm.url {
                    if let Some(fname) = net::fetch_favicon(url, &favicons_dir) {
                        let _ = s2.lock().unwrap().db.set_favicon(bm.id, &fname);
                    }
                }
                let done = i + 1;
                let s3 = s2.clone(); let w3 = w2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w3.unwrap();
                    if done == total {
                        let st = s3.lock().unwrap();
                        ui.set_bookmarks(st.build_bookmark_model()); ui.set_status_text(st.status());
                    } else {
                        ui.set_status_text(SharedString::from(format!("Favicon: {done}/{total}...")));
                    }
                });
            }
        });
        ui.set_status_text(SharedString::from(format!("Загрузка favicon для {total} ссылок...")));
    }); }

    // ── Check links ───────────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_check_links(move || {
        let ui = w.unwrap();
        let bms = { let st = s.lock().unwrap(); st.active_folder.map(|id| st.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default() };
        let total = bms.len();
        if total == 0 { ui.set_status_text(SharedString::from("Нет ссылок")); return; }
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            for (i, bm) in bms.into_iter().enumerate() {
                let result = bm.url.as_deref().map(net::check_url).unwrap_or((false, "no url".to_string()));
                let done = i + 1;
                let s3 = s2.clone(); let w3 = w2.clone(); let bm_id = bm.id;
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w3.unwrap();
                    s3.lock().unwrap().check_results.insert(bm_id, result);
                    if done == total {
                        let st = s3.lock().unwrap();
                        ui.set_bookmarks(st.build_bookmark_model()); ui.set_status_text(st.status());
                    } else {
                        ui.set_status_text(SharedString::from(format!("Проверка: {done}/{total}...")));
                    }
                });
            }
        });
        ui.set_status_text(SharedString::from(format!("Проверяю {total} ссылок...")));
    }); }

    ui.run().unwrap();
}
