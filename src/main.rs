#![windows_subsystem = "windows"]

mod compat; // Win7 compatibility shims (WaitOnAddress, ProcessPrng, etc.)
mod db;
mod net;
mod platform;

use db::Database;
use slint::{Image, ModelRc, SharedString, VecModel};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

slint::include_modules!();

// ── State ────────────────────────────────────────────────────────────────────

struct DetectedBrowser {
    name: String,
    kind: String,  // "chromium" | "firefox"
    path: String,
}

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
    col_name_width: i32,
    confirm_delete: bool,
    no_dup_urls: bool,
    dark_theme: bool,
    show_toolbar: bool,
    accordion: bool,
    favicon_cancel: Arc<std::sync::atomic::AtomicBool>,
    detected_browsers: Vec<DetectedBrowser>,
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy { Title, Url }

impl State {
    fn new(db: Database) -> Self {
        let data_dir = std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(std::path::Path::new(".")).join("Data");
        let (tree_width, col_name_width, confirm_delete, no_dup_urls, dark_theme, show_toolbar, accordion, expanded, active_folder) = State::load_settings();
        State {
            db, expanded, active_folder, selected_bookmark: None,
            search_query: String::new(), sort_by: SortBy::Title, sort_asc: true,
            data_dir, check_results: Default::default(), tree_width, col_name_width,
            confirm_delete, no_dup_urls, dark_theme, show_toolbar, accordion,
            favicon_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            detected_browsers: Vec::new(),
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
        let counts = self.db.get_all_bookmark_counts();
        let favicons_dir = self.favicons_dir();
        let mut result = Vec::new();
        self.walk_tree(&all_folders, &children, &favicons, &counts, &favicons_dir, None, 0, &mut result);
        ModelRc::new(VecModel::from(result))
    }

    fn walk_tree(&self,
        all: &[db::DbFolder],
        children: &std::collections::HashMap<Option<i64>, Vec<usize>>,
        favicons: &std::collections::HashMap<i64, String>,
        counts: &std::collections::HashMap<i64, i64>,
        favicons_dir: &std::path::Path,
        parent: Option<i64>, depth: i32,
        out: &mut Vec<TreeNode>)
    {
        if let Some(kids) = children.get(&parent) {
            for &i in kids {
                let f = &all[i];
                let count = counts.get(&f.id).copied().unwrap_or(0) as i32;
                let has_ch = children.contains_key(&Some(f.id)) || count > 0;
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
                    self.walk_tree(all, children, favicons, counts, favicons_dir, Some(f.id), depth + 1, out);

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

        // Search mode: show all matching bookmarks (no folder subfolders)
        if !self.search_query.is_empty() {
            let mut bms = self.db.search(&self.search_query).unwrap_or_default();
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
            return ModelRc::new(VecModel::from(items));
        }

        if let Some(folder_id) = self.active_folder {
            let all_folders = self.db.get_all_folders().unwrap_or_default();
            let counts = self.db.get_all_bookmark_counts();
            for f in all_folders.iter().filter(|f| f.parent_id == Some(folder_id)) {
                items.push(RightItem {
                    id: f.id as i32, kind: 0,
                    title: SharedString::from(f.title.as_str()),
                    url: SharedString::default(),
                    note: SharedString::default(),
                    favicon: Image::default(), has_favicon: false,
                    check_status: SharedString::default(),
                    count: counts.get(&f.id).copied().unwrap_or(0) as i32,
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

    fn bms_to_right_items(&self, bms: &[db::DbBookmark]) -> ModelRc<RightItem> {
        let favicons = self.db.get_favicons();
        let favicons_dir = self.favicons_dir();
        let items: Vec<RightItem> = bms.iter().map(|b| {
            let (fav_img, has_fav) = load_favicon(b.id, &favicons, &favicons_dir);
            let check_status = self.check_results.get(&b.id)
                .map(|(ok, code)| if *ok { "OK".to_string() } else { code.clone() })
                .unwrap_or_default();
            RightItem {
                id: b.id as i32, kind: 1,
                title: SharedString::from(b.title.as_str()),
                url: SharedString::from(b.url.as_deref().unwrap_or("")),
                note: SharedString::from(b.note.as_deref().unwrap_or("")),
                favicon: fav_img, has_favicon: has_fav,
                check_status: SharedString::from(check_status.as_str()),
                count: 0,
                selected: self.selected_bookmark == Some(b.id),
            }
        }).collect();
        ModelRc::new(VecModel::from(items))
    }

    fn sort_label(&self) -> SharedString {
        let arrow = if self.sort_asc { "↑" } else { "↓" };
        let by = match self.sort_by { SortBy::Title => "Название", SortBy::Url => "URL" };
        SharedString::from(format!("{by} {arrow}"))
    }

    fn status(&self) -> SharedString {
        let (_folders, bms) = self.db.total_counts();
        let db_name = self.db.path().file_name()
            .unwrap_or_default().to_string_lossy().to_string();
        if !self.search_query.is_empty() {
            let n = self.db.search(&self.search_query).unwrap_or_default().len();
            return SharedString::from(format!("Найдено: {n}  |  База: {db_name}"));
        }
        SharedString::from(format!("Записей: {bms}  |  База: {db_name}"))
    }

    fn breadcrumb(&self) -> SharedString {
        if !self.search_query.is_empty() {
            return SharedString::from(format!("Поиск: {}", self.search_query));
        }
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
        let cd = if self.confirm_delete { 1 } else { 0 };
        let nd = if self.no_dup_urls { 1 } else { 0 };
        let dt = if self.dark_theme { 1 } else { 0 };
        let st = if self.show_toolbar { 1 } else { 0 };
        let ac = if self.accordion { 1 } else { 0 };
        let json = format!(
            "{{\"tree_width\":{},\"col_name_width\":{},\"confirm_delete\":{cd},\"no_dup_urls\":{nd},\"dark_theme\":{dt},\"show_toolbar\":{st},\"accordion\":{ac},\"expanded\":\"{exp_str}\",\"active_folder\":{active}}}",
            self.tree_width, self.col_name_width);
        let _ = std::fs::write(Self::settings_path(), json.as_bytes());
    }

    fn load_settings() -> (f32, i32, bool, bool, bool, bool, bool, HashSet<i64>, Option<i64>) {
        let mut width = 240.0f32;
        let mut col_w = 240i32;
        let mut confirm_delete = true;
        let mut no_dup_urls = false;
        let mut dark_theme = false;
        let mut show_toolbar = true;
        let mut accordion = false;
        let mut expanded = HashSet::new();
        let mut active: Option<i64> = None;
        if let Ok(s) = std::fs::read_to_string(Self::settings_path()) {
            if let Some(p) = s.find("\"tree_width\":") {
                let rest = &s[p+13..];
                let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
                if let Ok(v) = rest[..end].parse::<f32>() { width = v.max(100.0).min(500.0); }
            }
            if let Some(p) = s.find("\"col_name_width\":") {
                let rest = &s[p+17..];
                let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                if let Ok(v) = rest[..end].parse::<i32>() { col_w = v.max(80).min(700); }
            }
            if let Some(p) = s.find("\"confirm_delete\":") {
                let rest = &s[p+17..].trim_start();
                confirm_delete = rest.starts_with('1') || rest.starts_with("true");
            }
            if let Some(p) = s.find("\"no_dup_urls\":") {
                let rest = &s[p+14..].trim_start();
                no_dup_urls = rest.starts_with('1') || rest.starts_with("true");
            }
            if let Some(p) = s.find("\"dark_theme\":") {
                let rest = &s[p+13..].trim_start();
                dark_theme = rest.starts_with('1') || rest.starts_with("true");
            }
            if let Some(p) = s.find("\"show_toolbar\":") {
                let rest = &s[p+15..].trim_start();
                show_toolbar = rest.starts_with('1') || rest.starts_with("true");
            }
            if let Some(p) = s.find("\"accordion\":") {
                let rest = &s[p+12..].trim_start();
                accordion = rest.starts_with('1') || rest.starts_with("true");
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
        (width, col_w, confirm_delete, no_dup_urls, dark_theme, show_toolbar, accordion, expanded, active)
    }

    fn exe_dir() -> std::path::PathBuf {
        std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(std::path::Path::new(".")).to_path_buf()
    }

    fn recent_dbs_path() -> std::path::PathBuf { Self::exe_dir().join("recent_dbs.txt") }

    fn save_recent_db(path: &std::path::Path) {
        let mut recent = Self::load_recent_dbs();
        let p = path.to_string_lossy().to_string();
        recent.retain(|x| x != &p);
        recent.insert(0, p);
        recent.truncate(10);
        let _ = std::fs::write(Self::recent_dbs_path(), recent.join("\n"));
    }

    fn load_recent_dbs() -> Vec<String> {
        std::fs::read_to_string(Self::recent_dbs_path())
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
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

/// Collect visible tree nodes in display order for DnD hit-testing: (id, kind)
fn collect_tree_order(
    all: &[db::DbFolder],
    children: &std::collections::HashMap<Option<i64>, Vec<usize>>,
    expanded: &std::collections::HashSet<i64>,
    parent: Option<i64>,
    out: &mut Vec<(i64, i32)>,  // (id, kind: 0=folder 1=bookmark)
) {
    if let Some(kids) = children.get(&parent) {
        for &i in kids {
            let f = &all[i];
            out.push((f.id, 0));
            if expanded.contains(&f.id) {
                collect_tree_order(all, children, expanded, Some(f.id), out);
                // We don't include bookmark leaves for simplicity (only folders are drop targets)
            }
        }
    }
}

/// Group bookmarks by domain — returns (representative_bm, all_ids_with_same_domain).
fn dedup_by_domain(bms: Vec<db::DbBookmark>) -> Vec<(db::DbBookmark, Vec<i64>)> {
    let mut map: std::collections::HashMap<String, (db::DbBookmark, Vec<i64>)> = Default::default();
    let mut no_domain: Vec<(db::DbBookmark, Vec<i64>)> = Vec::new();
    for bm in bms {
        if let Some(url) = bm.url.as_deref() {
            if let Some(domain) = net::extract_domain(url) {
                let entry = map.entry(domain).or_insert_with(|| (bm.clone(), Vec::new()));
                entry.1.push(bm.id);
                continue;
            }
        }
        no_domain.push((bm.clone(), vec![bm.id]));
    }
    let mut result: Vec<_> = map.into_values().collect();
    result.extend(no_domain);
    result
}

fn refresh_ui(ui: &MainWindow, st: &State) {
    ui.set_tree_nodes(st.build_tree_model());
    ui.set_right_items(st.build_right_panel_model());
    ui.set_status_text(st.status());
    ui.set_sort_label(st.sort_label());
    ui.set_breadcrumb(st.breadcrumb());
    // Sync selection properties
    ui.set_active_folder_id(st.active_folder.unwrap_or(0) as i32);
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
            let created = bm.created.as_deref().unwrap_or("").to_string();
            let created_display = if created.len() >= 10 { created[..10].to_string() } else { created };
            ui.set_detail_created(SharedString::from(created_display.as_str()));
            ui.set_detail_favicon(fav_img);
            ui.set_detail_has_favicon(has_fav);
            // Load thumbnail if exists
            let (thumb_img, has_thumb) = if let Some(path) = bm.thumb.as_deref() {
                if !path.is_empty() {
                    let p = std::path::Path::new(path);
                    if p.exists() {
                        if let Ok(img) = Image::load_from_path(p) { (img, true) }
                        else { (Image::default(), false) }
                    } else { (Image::default(), false) }
                } else { (Image::default(), false) }
            } else { (Image::default(), false) };
            ui.set_detail_thumb(thumb_img);
            ui.set_detail_has_thumb(has_thumb);
            ui.set_active_bookmark(id as i32);
            return;
        }
    }
    ui.set_detail_title(SharedString::default());
    ui.set_detail_url(SharedString::default());
    ui.set_detail_note(SharedString::default());
    ui.set_detail_created(SharedString::default());
    ui.set_detail_favicon(Image::default());
    ui.set_detail_has_favicon(false);
    ui.set_detail_thumb(Image::default());
    ui.set_detail_has_thumb(false);
    ui.set_active_bookmark(0);
}

/// Expands all ancestor folders of a bookmark in the tree so the bookmark
/// becomes visible after build_tree_model() is called.
/// Equivalent to URL-Album-2's expandTreePath(parentFolderId) + _activateTreeItem.
/// Errors from DB are silently skipped — the detail view still opens.
// TODO: scroll tree ScrollView to selected bookmark when out of viewport.
// Requires computing Y position from variable-height tree-nodes — separate task.
fn expand_path_to(st: &mut State, bookmark_id: i64) {
    let mut cur = st.db.get_node_parent(bookmark_id);
    while let Some(folder_id) = cur {
        st.expanded.insert(folder_id);
        cur = st.db.get_node_parent(folder_id);
    }
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
    State::save_recent_db(path);
}

fn detect_browsers() -> Vec<DetectedBrowser> {
    let local   = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    let pf      = std::env::var("PROGRAMFILES").unwrap_or_default();
    let pf86    = std::env::var("PROGRAMFILES(X86)").unwrap_or_else(|_| pf.clone());
    let mut out: Vec<DetectedBrowser> = Vec::new();

    let chromium_apps: &[(&str, &str)] = &[
        ("Google Chrome",  "Google\\Chrome"),
        ("Microsoft Edge", "Microsoft\\Edge"),
        ("Brave",          "BraveSoftware\\Brave-Browser"),
        ("Vivaldi",        "Vivaldi"),
        ("Chromium",       "Chromium"),
    ];
    for (name, rel) in chromium_apps {
        'bases: for base in &[local.as_str(), pf.as_str(), pf86.as_str()] {
            for path in chromium_bookmark_paths(base, rel) {
                if std::path::Path::new(&path).exists() && !out.iter().any(|b| b.path == path) {
                    out.push(DetectedBrowser { name: name.to_string(), kind: "chromium".to_string(), path });
                    break 'bases;
                }
            }
        }
    }

    for base in &[roaming.as_str(), local.as_str()] {
        let opera_base = format!("{}\\Opera Software", base);
        if let Ok(entries) = std::fs::read_dir(&opera_base) {
            for entry in entries.filter_map(|e| e.ok()) {
                if !entry.path().is_dir() { continue; }
                let profile_dir = entry.path();
                for rel in &["Bookmarks", "Default\\Bookmarks", "User Data\\Default\\Bookmarks"] {
                    let p = profile_dir.join(rel);
                    if p.exists() {
                        let path = p.to_string_lossy().into_owned();
                        if !out.iter().any(|b| b.path == path) {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            out.push(DetectedBrowser { name, kind: "chromium".to_string(), path });
                        }
                        break;
                    }
                }
            }
        }
    }

    let ff_browsers: &[(&str, &str)] = &[
        ("Mozilla\\Firefox", "Mozilla Firefox"),
        ("Waterfox",         "Waterfox"),
        ("LibreWolf",        "LibreWolf"),
        ("Pale Moon",        "Pale Moon"),
        ("SeaMonkey",        "SeaMonkey"),
    ];
    for (rel, name) in ff_browsers {
        let base = format!("{}\\{}", roaming, rel);
        if let Some(places) = find_gecko_places(&base) {
            if !out.iter().any(|b| b.path == places) {
                out.push(DetectedBrowser { name: name.to_string(), kind: "firefox".to_string(), path: places });
            }
        }
    }
    out
}

fn chromium_bookmark_paths(base: &str, app_name: &str) -> Vec<String> {
    vec![
        format!("{}\\{}\\User Data\\Default\\Bookmarks", base, app_name),
        format!("{}\\{}\\Default\\Bookmarks", base, app_name),
        format!("{}\\{}\\Bookmarks", base, app_name),
    ]
}

fn find_gecko_places(browser_base: &str) -> Option<String> {
    let ini = std::fs::read_to_string(format!("{}\\profiles.ini", browser_base)).ok()?;
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

fn browser_letter_color(name: &str, kind: &str) -> (String, slint::Color) {
    let nl = name.to_lowercase();
    if kind == "firefox" { return ("?".to_string(), slint::Color::from_rgb_u8(136, 136, 136)); }
    if nl.contains("chrome") || nl.contains("chromium") { return ("C".to_string(), slint::Color::from_rgb_u8(26, 115, 232)); }
    if nl.contains("edge") { return ("E".to_string(), slint::Color::from_rgb_u8(0, 120, 212)); }
    if nl.contains("opera") { return ("O".to_string(), slint::Color::from_rgb_u8(255, 27, 45)); }
    if nl.contains("brave") { return ("B".to_string(), slint::Color::from_rgb_u8(251, 84, 43)); }
    if nl.contains("vivaldi") { return ("V".to_string(), slint::Color::from_rgb_u8(239, 57, 57)); }
    let letter = name.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_else(|| "?".to_string());
    (letter, slint::Color::from_rgb_u8(85, 85, 85))
}

// ── Favicon batch timer (runs on main thread via slint::Timer) ───────────────

fn start_favicon_timer(
    ui_weak: slint::Weak<MainWindow>,
    state: Arc<Mutex<State>>,
    done_count: Arc<AtomicUsize>,
    needs_rebuild: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
    total: usize,
) {
    let mut last_n = 0usize;
    platform::set_frame_callback(move || {
        let ui = match ui_weak.upgrade() { Some(u) => u, None => return };
        let n = done_count.load(Ordering::Relaxed).min(total);
        if n != last_n {
            last_n = n;
            ui.set_favicon_progress_text(SharedString::from(format!("Favicon: {n} / {total}")));
            ui.set_favicon_progress_value(n as f32 / total as f32);
        }
        if needs_rebuild.swap(false, Ordering::Relaxed) {
            let st = state.lock().unwrap();
            ui.set_tree_nodes(st.build_tree_model());
            ui.set_right_items(st.build_right_panel_model());
        }
        if n >= total || cancel_flag.load(Ordering::Relaxed) {
            ui.set_show_favicon_progress(false);
            let st = state.lock().unwrap();
            ui.set_status_text(st.status());
            platform::clear_frame_callback();
        }
    });
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    compat::init(); // keep Win7 shim statics alive through LTO
    // Install custom Win32 platform (Win7+ compatible, no WinRT)
    let win32_platform = platform::Win32Platform::new("URL Album 3")
        .expect("Failed to create Win32 platform");
    slint::platform::set_platform(Box::new(win32_platform))
        .expect("Platform already set");

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
        ui.set_col_name_width_px(st.col_name_width);
        ui.set_dark_theme(st.dark_theme);
        ui.set_show_toolbar(st.show_toolbar);
        ui.set_setting_dark_theme(st.dark_theme);
        ui.set_setting_show_toolbar(st.show_toolbar);
        ui.set_setting_accordion(st.accordion);
        ui.set_setting_confirm_delete(st.confirm_delete);
        ui.set_setting_no_dup_urls(st.no_dup_urls);
        let db_name = db_path.file_name().unwrap_or_default().to_string_lossy().to_string();
        ui.set_db_name(SharedString::from(db_name.as_str()));
    }
    refresh_ui(&ui, &state.lock().unwrap());

    // ── Tree navigation ───────────────────────────────────────────────────

    // Folder clicked → только обновить правую панель и выделение,
    // ДЕРЕВО НЕ ПЕРЕСТРАИВАЕМ — иначе двойной клик не работает
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_tree_folder_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64);
        st.selected_bookmark = None;
        st.save_settings();
        // Выделение через свойство — без перестройки дерева
        ui.set_active_folder_id(id);
        ui.set_active_bookmark(0);
        ui.set_right_items(st.build_right_panel_model());
        ui.set_status_text(st.status());
        ui.set_breadcrumb(st.breadcrumb());
        ui.set_sort_label(st.sort_label());
        update_detail(&ui, &st);
    }); }

    // Folder toggled (double-click or [+]/[-]) — also selects the folder
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_tree_folder_toggled(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let fid = id as i64;
        if st.expanded.contains(&fid) { st.expanded.remove(&fid); } else { st.expanded.insert(fid); }
        st.active_folder = Some(fid);
        st.selected_bookmark = None;
        st.save_settings();
        ui.set_active_folder_id(id);
        ui.set_active_bookmark(0);
        ui.set_tree_nodes(st.build_tree_model());
        ui.set_right_items(st.build_right_panel_model());
        ui.set_status_text(st.status());
        ui.set_breadcrumb(st.breadcrumb());
        update_detail(&ui, &st);
    }); }

    // Bookmark clicked in tree → expand ancestor folders, show detail
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_tree_bookmark_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        expand_path_to(&mut st, id as i64);
        ui.set_tree_nodes(st.build_tree_model());
        update_detail(&ui, &st); }); }

    // Back to list mode (Escape or "← Назад")
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_back_to_list(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = None;
        ui.set_active_bookmark(0);
        ui.set_tree_nodes(st.build_tree_model()); }); }

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
        update_detail(&ui, &st); }); }

    // Navigate into subfolder from right panel
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_right_folder_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        st.expanded.insert(id as i64); st.save_settings();
        refresh_ui(&ui, &st); }); }

    // Bookmark clicked in right panel → expand ancestor folders in tree, show detail
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_right_bookmark_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.selected_bookmark = Some(id as i64);
        expand_path_to(&mut st, id as i64);
        ui.set_tree_nodes(st.build_tree_model());
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
        let url_str = url.as_str();
        // noDuplicateUrls check
        if st.no_dup_urls && !url_str.is_empty() {
            if let Ok(existing) = st.db.search(url_str) {
                if existing.iter().any(|b| b.url.as_deref() == Some(url_str)) {
                    ui.set_status_text(SharedString::from(format!("Дублирующийся URL: {url_str}")));
                    return;
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
        } else if let Some(id) = st.active_folder {
            if let Some(title) = st.db.get_folder_title(id) {
                ui.set_rename_prefill(SharedString::from(title.as_str()));
                ui.set_show_rename_dlg(true);
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
        if st.selected_bookmark.is_none() && st.active_folder.is_none() { return; }
        if !st.confirm_delete {
            // Delete immediately without dialog
            drop(st);
            let ui2 = w.unwrap(); let mut st2 = s.lock().unwrap();
            if let Some(id) = st2.selected_bookmark {
                let _ = st2.db.delete_bookmark(id); st2.selected_bookmark = None;
                ui2.set_active_bookmark(0);
            } else if let Some(id) = st2.active_folder {
                let _ = st2.db.delete_folder(id); st2.expanded.remove(&id); st2.active_folder = None;
                st2.save_settings();
            }
            refresh_ui(&ui2, &st2);
            return;
        }
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
        let bms = { let st = s.lock().unwrap(); st.db.get_bookmarks_recursive(folder_id as i64) };
        let favicons_dir = { s.lock().unwrap().favicons_dir() };
        let deduped = dedup_by_domain(bms);
        let total = deduped.len(); if total == 0 { return; }
        { s.lock().unwrap().favicon_cancel.store(false, Ordering::Relaxed); }
        ui.set_show_favicon_progress(true);
        ui.set_favicon_progress_text(SharedString::from(format!("Favicon: 0 / {total}")));
        ui.set_favicon_progress_value(0.0);
        let cancel_flag = s.lock().unwrap().favicon_cancel.clone();
        let queue = Arc::new(Mutex::new(deduped));
        let done_count = Arc::new(AtomicUsize::new(0));
        let needs_rebuild = Arc::new(AtomicBool::new(false));
        for _ in 0..5 {
            let queue = queue.clone(); let done_count = done_count.clone();
            let needs_rebuild = needs_rebuild.clone();
            let s2 = s.clone();
            let favicons_dir = favicons_dir.clone(); let cancel = cancel_flag.clone();
            std::thread::spawn(move || { loop {
                if cancel.load(Ordering::Relaxed) { break; }
                let item = { queue.lock().unwrap().pop() };
                let Some((bm, same_ids)) = item else { break; };
                if let Some(url) = bm.url.as_deref() {
                    if let Some(fname) = net::fetch_favicon(url, &favicons_dir) {
                        let st = s2.lock().unwrap();
                        for id in &same_ids { let _ = st.db.set_favicon(*id, &fname); }
                        needs_rebuild.store(true, Ordering::Relaxed);
                    }
                }
                done_count.fetch_add(1, Ordering::Relaxed);
            }});
        }
        start_favicon_timer(ui.as_weak(), s.clone(), done_count, needs_rebuild, cancel_flag, total);
    }); }

    // Single bookmark favicon load
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_load_favicon_bm(move |id| {
        let _ui = w.unwrap();
        let (url, favicons_dir) = { let st = s.lock().unwrap();
            (st.db.get_bookmark(id as i64).and_then(|b| b.url), st.favicons_dir()) };
        let Some(url) = url else { return; };
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            if let Some(fname) = net::fetch_favicon(&url, &favicons_dir) {
                let _ = s2.lock().unwrap().db.set_favicon(id as i64, &fname);
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w2.unwrap();
                    let st = s2.lock().unwrap();
                    ui.set_tree_nodes(st.build_tree_model());
                    ui.set_right_items(st.build_right_panel_model());
                    ui.set_status_text(SharedString::from("Favicon загружен"));
                });
            }
        });
    }); }

    // Sort folder from context menu
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_sort_folder(move |id| {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        let _ = st.db.sort_folder(id as i64);
        refresh_ui(&ui, &st);
    }); }

    // Check links in folder from context menu
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_ctx_check_links_folder(move |folder_id| {
        let _ui = w.unwrap();
        let bms = { let st = s.lock().unwrap(); st.db.get_bookmarks(folder_id as i64).unwrap_or_default() };
        let total = bms.len(); if total == 0 { return; }
        let s2 = s.clone(); let w2 = w.clone();
        std::thread::spawn(move || {
            for bm in bms {
                let result = bm.url.as_deref().map(net::check_url).unwrap_or((false, "no url".into()));
                let bm_id = bm.id; let s3 = s2.clone(); let w3 = w2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let ui = w3.unwrap(); s3.lock().unwrap().check_results.insert(bm_id, result);
                    let st = s3.lock().unwrap();
                    ui.set_right_items(st.build_right_panel_model());
                });
            }
            let _s3 = s2.clone(); let w3 = w2.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let ui = w3.unwrap();
                ui.set_status_text(SharedString::from(format!("Проверено {total} ссылок")));
            });
        });
    }); }

    // ── Move item up/down (сортировка внутри папки) ───────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_move_item_up(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        let id = st.selected_bookmark.or(st.active_folder);
        if let Some(id) = id {
            let _ = st.db.move_item_relative(id, -1);
            drop(st); refresh_ui(&ui, &s.lock().unwrap());
        }
    }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_move_item_down(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        let id = st.selected_bookmark.or(st.active_folder);
        if let Some(id) = id {
            let _ = st.db.move_item_relative(id, 1);
            drop(st); refresh_ui(&ui, &s.lock().unwrap());
        }
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
        ui.set_sort_label(st.sort_label()); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_search_changed(move |query| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.search_query = query.to_string(); st.selected_bookmark = None;
        ui.set_active_bookmark(0);
        ui.set_right_items(st.build_right_panel_model());
        ui.set_breadcrumb(st.breadcrumb());
        ui.set_status_text(st.status()); }); }

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
            let st = s.lock().unwrap();
            match st.db.import_uadat(&path) {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт ua.dat: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_html(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("HTML", &["html","htm"]).add_filter("All", &["*"]).pick_file() {
            let st = s.lock().unwrap();
            match st.db.import_html(&path) {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт HTML: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_browser(move || {
        let ui = w.unwrap();
        let browsers = detect_browsers();
        let entries: Vec<BrowserEntry> = browsers.iter().map(|b| {
            let (letter, color) = browser_letter_color(&b.name, &b.kind);
            BrowserEntry { name: SharedString::from(b.name.as_str()), letter: SharedString::from(letter), letter_color: color }
        }).collect();
        s.lock().unwrap().detected_browsers = browsers;
        ui.set_browser_list(ModelRc::new(VecModel::from(entries)));
        ui.set_browser_selected_idx(-1);
        ui.set_show_browser_import_dlg(true);
      }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_browser_import_ok(move |idx| {
        let ui = w.unwrap();
        let (name, kind, path) = {
            let st = s.lock().unwrap();
            let i = idx as usize;
            if i >= st.detected_browsers.len() { return; }
            let b = &st.detected_browsers[i];
            (b.name.clone(), b.kind.clone(), b.path.clone())
        };
        let pb = std::path::PathBuf::from(&path);
        let st = s.lock().unwrap();
        let result = if kind == "firefox" {
            st.db.import_firefox(&pb, &name).map_err(|e| e.to_string())
        } else {
            st.db.import_chrome_json_named(&pb, &name).map_err(|e| e.to_string())
        };
        match result {
            Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт {}: {n} ссылок", name))); refresh_ui(&ui, &st); }
            Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
        }
      }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_browser_file_pick(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Выбрать файл закладок")
            .add_filter("Файлы закладок", &["json", "Bookmarks", "sqlite"])
            .add_filter("Все файлы", &["*"])
            .pick_file()
        {
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
            let st = s.lock().unwrap();
            let result = if fname == "places.sqlite" || fname.ends_with(".sqlite") {
                st.db.import_firefox(&path, "Firefox Import").map_err(|e| e.to_string())
            } else {
                st.db.import_chrome_json_named(&path, "Browser Import").map_err(|e| e.to_string())
            };
            match result {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        }
      }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_browser_folder_pick(move || {
        let ui = w.unwrap();
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Выбрать папку профиля браузера")
            .pick_folder()
        {
            let mut found: Option<(std::path::PathBuf, bool)> = None;
            for rel in &["Bookmarks", "Default\\Bookmarks", "User Data\\Default\\Bookmarks"] {
                let p = folder.join(rel);
                if p.exists() { found = Some((p, false)); break; }
            }
            if found.is_none() {
                for rel in &["places.sqlite", "default\\places.sqlite"] {
                    let p = folder.join(rel);
                    if p.exists() { found = Some((p, true)); break; }
                }
            }
            match found {
                None => ui.set_status_text(SharedString::from("Файл закладок не найден в папке")),
                Some((path, is_ff)) => {
                    let st = s.lock().unwrap();
                    let result = if is_ff {
                        st.db.import_firefox(&path, "Firefox Import").map_err(|e| e.to_string())
                    } else {
                        st.db.import_chrome_json_named(&path, "Browser Import").map_err(|e| e.to_string())
                    };
                    match result {
                        Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт: {n} ссылок"))); refresh_ui(&ui, &st); }
                        Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
                    }
                }
            }
        }
      }); }

    // ── Favicon loading ───────────────────────────────────────────────────────

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_load_favicons(move || {
        let ui = w.unwrap();
        let (bms, favicons_dir) = { let st = s.lock().unwrap();
            (st.active_folder.map(|id| st.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default(), st.favicons_dir()) };
        let deduped = dedup_by_domain(bms);
        let total = deduped.len();
        if total == 0 { ui.set_status_text(SharedString::from("Нет ссылок для загрузки favicon")); return; }
        { s.lock().unwrap().favicon_cancel.store(false, Ordering::Relaxed); }
        ui.set_show_favicon_progress(true);
        ui.set_favicon_progress_text(SharedString::from(format!("Favicon: 0 / {total}")));
        ui.set_favicon_progress_value(0.0);
        let cancel_flag = s.lock().unwrap().favicon_cancel.clone();
        let queue = Arc::new(Mutex::new(deduped));
        let done_count = Arc::new(AtomicUsize::new(0));
        let needs_rebuild = Arc::new(AtomicBool::new(false));
        for _ in 0..5 {
            let queue = queue.clone(); let done_count = done_count.clone();
            let needs_rebuild = needs_rebuild.clone();
            let s2 = s.clone();
            let favicons_dir = favicons_dir.clone(); let cancel = cancel_flag.clone();
            std::thread::spawn(move || { loop {
                if cancel.load(Ordering::Relaxed) { break; }
                let item = { queue.lock().unwrap().pop() };
                let Some((bm, same_ids)) = item else { break; };
                if let Some(url) = bm.url.as_deref() {
                    if let Some(fname) = net::fetch_favicon(url, &favicons_dir) {
                        let st = s2.lock().unwrap();
                        for id in &same_ids { let _ = st.db.set_favicon(*id, &fname); }
                        needs_rebuild.store(true, Ordering::Relaxed);
                    }
                }
                done_count.fetch_add(1, Ordering::Relaxed);
            }});
        }
        start_favicon_timer(ui.as_weak(), s.clone(), done_count, needs_rebuild, cancel_flag, total);
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
                    if done == total {
                        let st = s3.lock().unwrap();
                        ui.set_right_items(st.build_right_panel_model());
                        ui.set_status_text(st.status());
                    } else {
                        ui.set_status_text(SharedString::from(format!("Проверка: {done}/{total}...")));
                    }
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
        let items = st.bms_to_right_items(&dups);
        ui.set_active_bookmark(0);
        ui.set_right_items(items);
        ui.set_breadcrumb(SharedString::from(format!("Дубликаты ({n})")));
        ui.set_tree_nodes(st.build_tree_model());
        ui.set_status_text(SharedString::from(format!("Дубликатов: {n} — удалите лишние (Del)"))); }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_show_all(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let all = st.db.get_all_bookmarks().unwrap_or_default(); let n = all.len();
        st.active_folder = None; st.selected_bookmark = None;
        let items = st.bms_to_right_items(&all);
        ui.set_active_bookmark(0);
        ui.set_right_items(items);
        ui.set_breadcrumb(SharedString::from(format!("Все ссылки ({n})")));
        ui.set_tree_nodes(st.build_tree_model());
        ui.set_status_text(SharedString::from(format!("Все ссылки: {n}"))); }); }

    { let s = state.clone();
      ui.on_tree_width_changed(move |w| {
        let mut st = s.lock().unwrap(); st.tree_width = w as f32; st.save_settings(); }); }

    { let s = state.clone();
      ui.on_col_name_width_changed(move |w| {
        let mut st = s.lock().unwrap(); st.col_name_width = w; st.save_settings(); }); }

    // Cancel favicon loading
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_cancel_favicon(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        st.favicon_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        ui.set_show_favicon_progress(false);
        ui.set_status_text(SharedString::from("Загрузка favicon отменена")); }); }

    // Expand all folders
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_expand_all(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let all = st.db.get_all_folders().unwrap_or_default();
        for f in &all { st.expanded.insert(f.id); }
        st.save_settings(); ui.set_all_expanded(true); ui.set_tree_nodes(st.build_tree_model()); }); }

    // Collapse all folders
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_collapse_all(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.expanded.clear(); st.save_settings();
        ui.set_all_expanded(false); ui.set_tree_nodes(st.build_tree_model()); }); }

    { let w = ui.as_weak();
      ui.on_focus_search(move || {
        // The LineEdit focus is handled by Slint's FocusScope; just trigger keyboard focus
        let _ = w; // placeholder - actual focus handled in Slint keyboard handler
    }); }

    // ── Drag & Drop ───────────────────────────────────────────────────────────

    // Вспомогательная функция — строит список видимых узлов дерева
    fn build_tree_order(st: &State) -> Vec<(i64, i32)> {
        let all_folders = st.db.get_all_folders().unwrap_or_default();
        let mut children: std::collections::HashMap<Option<i64>, Vec<usize>> = Default::default();
        for (i, f) in all_folders.iter().enumerate() {
            children.entry(f.parent_id).or_default().push(i);
        }
        let mut order: Vec<(i64, i32)> = Vec::new();
        collect_tree_order(&all_folders, &children, &st.expanded, None, &mut order);
        order
    }

    // Обновляем drag-target-id по позиции курсора в дереве (относительный Y)
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_drag_hovered(move |rel_y| {
        let ui = w.unwrap();
        let st = s.lock().unwrap();
        let order = build_tree_order(&st);
        let index = (rel_y / 22.0).max(0.0) as usize;
        let target = order.get(index)
            .filter(|(_, kind)| *kind == 0)
            .map(|(id, _)| *id as i32)
            .unwrap_or(0);
        ui.set_drag_target_id(target);
    }); }

    // Обновляем drag-target-id по АБСОЛЮТНОМУ Y (для DnD из правой панели)
    // Дерево начинается примерно на Y=56px (menubar 24 + toolbar 30 + 2px отступ)
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_drag_hovered_global(move |abs_y| {
        let ui = w.unwrap();
        let st = s.lock().unwrap();
        let tree_top: f32 = 58.0; // приблизительный Y начала дерева
        let rel_y = abs_y - tree_top;
        if rel_y < 0.0 { ui.set_drag_target_id(0); return; }
        let order = build_tree_order(&st);
        let index = (rel_y / 22.0) as usize;
        let target = order.get(index)
            .filter(|(_, kind)| *kind == 0)
            .map(|(id, _)| *id as i32)
            .unwrap_or(0);
        ui.set_drag_target_id(target);
    }); }

    // Выполняем перемещение после отпускания
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_drag_drop_item(move |drag_id, target_id| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        if drag_id == 0 || target_id == 0 || drag_id == target_id { return; }
        // Проверяем что не перемещаем в собственного потомка
        let _ = st.db.move_node(drag_id as i64, target_id as i64);
        st.expanded.insert(target_id as i64);
        st.save_settings();
        refresh_ui(&ui, &st);
    }); }

    // ── Exit ──────────────────────────────────────────────────────────────────
    ui.on_app_exit(|| { std::process::exit(0); });

    // ── Close DB (закрыть базу без выхода) ────────────────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_close_db(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.db = Database::open_default().unwrap_or_else(|_| st.db.clone_empty());
        st.expanded.clear(); st.active_folder = None; st.selected_bookmark = None;
        st.search_query.clear(); st.check_results.clear(); st.save_settings();
        ui.set_db_name(SharedString::from("(нет базы)"));
        ui.set_active_bookmark(0);
        refresh_ui(&ui, &st);
    }); }

    // ── Save / WAL checkpoint (Ctrl+S) ────────────────────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_save_db(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        let _ = st.db.checkpoint();
        ui.set_status_text(SharedString::from("База сохранена"));
    }); }

    // ── Copy selected URL (Ctrl+C) ────────────────────────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_copy_selected_url(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        let url = if let Some(id) = st.selected_bookmark {
            st.db.get_bookmark(id).and_then(|b| b.url).unwrap_or_default()
        } else { String::new() };
        if !url.is_empty() {
            copy_to_clipboard(&url);
            ui.set_status_text(SharedString::from(format!("Скопировано: {url}")));
        }
    }); }

    // ── Check selected link (F10) ─────────────────────────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_check_selected_link(move || {
        let _ui = w.unwrap();
        let url = { let st = s.lock().unwrap();
            st.selected_bookmark.and_then(|id| st.db.get_bookmark(id)).and_then(|b| b.url)
        };
        if let Some(url) = url {
            let w2 = w.clone();
            std::thread::spawn(move || {
                let (ok, code) = net::check_url(&url);
                let msg = if ok { format!("OK {code}: {url}") } else { format!("Ошибка {code}: {url}") };
                let _ = slint::invoke_from_event_loop(move || {
                    w2.unwrap().set_status_text(SharedString::from(msg.as_str()));
                });
            });
        }
    }); }

    // ── Open with (Открыть с помощью) ─────────────────────────────────────────
    { let s = state.clone();
      ui.on_open_selected_with(move || {
        let st = s.lock().unwrap();
        let url = st.selected_bookmark
            .and_then(|id| st.db.get_bookmark(id))
            .and_then(|b| b.url)
            .unwrap_or_default();
        if !url.is_empty() {
            // Открываем диалог "Открыть с помощью" Windows
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("rundll32.exe")
                .args(["shell32.dll,OpenAs_RunDLL", &url])
                .creation_flags(0x0800_0000).spawn();
        }
    }); }

    // ── Settings ─────────────────────────────────────────────────────────────
    {
        let st = state.lock().unwrap();
        ui.set_setting_confirm_delete(st.confirm_delete);
        ui.set_setting_no_dup_urls(st.no_dup_urls);
    }
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_settings_ok(move |confirm_del, no_dup, dark_theme, show_toolbar, accordion| {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        st.confirm_delete = confirm_del;
        st.no_dup_urls = no_dup;
        st.dark_theme = dark_theme;
        st.show_toolbar = show_toolbar;
        st.accordion = accordion;
        st.save_settings();
        // Apply theme + toolbar immediately
        ui.set_dark_theme(dark_theme);
        ui.set_show_toolbar(show_toolbar);
    }); }

    // ── DB Properties ─────────────────────────────────────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_show_db_props(move || {
        let ui = w.unwrap(); let st = s.lock().unwrap();
        let path = st.db.path();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let folders = st.db.get_all_folders().unwrap_or_default().len();
        let bookmarks = st.db.get_all_bookmarks().unwrap_or_default().len();
        fn fmt_bytes(b: u64) -> String {
            if b < 1024 { format!("{b} Б") }
            else if b < 1024*1024 { format!("{:.1} КБ", b as f64/1024.0) }
            else { format!("{:.2} МБ", b as f64/1024.0/1024.0) }
        }
        ui.set_dbprops_path(SharedString::from(path.to_string_lossy().as_ref()));
        ui.set_dbprops_size(SharedString::from(fmt_bytes(size).as_str()));
        ui.set_dbprops_folders(SharedString::from(folders.to_string().as_str()));
        ui.set_dbprops_bookmarks(SharedString::from(bookmarks.to_string().as_str()));
        ui.set_show_dbprops_dlg(true);
    }); }

    { let s = state.clone(); let w = ui.as_weak();
      ui.on_clear_db(move || {
        let ui = w.unwrap(); let mut st = s.lock().unwrap();
        let _ = st.db.clear();
        st.expanded.clear(); st.active_folder = None; st.selected_bookmark = None;
        st.search_query.clear(); st.check_results.clear(); st.save_settings();
        ui.set_active_bookmark(0);
        refresh_ui(&ui, &st);
    }); }

    // ── Recent databases ──────────────────────────────────────────────────────
    {
        let recent = State::load_recent_dbs();
        let model: Vec<SharedString> = recent.iter().map(|s| SharedString::from(s.as_str())).collect();
        ui.set_recent_dbs(ModelRc::new(VecModel::from(model)));
    }
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_open_db_path(move |path| {
        let ui = w.unwrap();
        let p = std::path::PathBuf::from(path.as_str());
        match Database::open_at(&p) {
            Ok(new_db) => {
                let _ = new_db.init_schema();
                let mut st = s.lock().unwrap();
                st.db = new_db; st.expanded.clear(); st.active_folder = None;
                st.selected_bookmark = None; st.search_query.clear(); st.check_results.clear();
                st.save_settings(); save_last_db(&p);
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                ui.set_db_name(SharedString::from(name.as_str())); ui.set_active_bookmark(0);
                // Refresh recent list
                let recent = State::load_recent_dbs();
                let model: Vec<SharedString> = recent.iter().map(|s| SharedString::from(s.as_str())).collect();
                ui.set_recent_dbs(ModelRc::new(VecModel::from(model)));
                refresh_ui(&ui, &st);
            }
            Err(e) => { ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))); }
        }
    }); }

    // ── Import TXT ────────────────────────────────────────────────────────────
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_txt(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Text", &["txt"]).add_filter("All", &["*"]).pick_file() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let st = s.lock().unwrap();
                let parent = st.active_folder;
                let mut count = 0usize;
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') { continue; }
                    let (url, title) = if line.contains('\t') {
                        let mut parts = line.splitn(2, '\t');
                        (parts.next().unwrap_or("").trim(), parts.next().unwrap_or("").trim())
                    } else { (line, line) };
                    if url.is_empty() { continue; }
                    let title = if title.is_empty() { url } else { title };
                    if let Ok(id) = st.db.create_bookmark(parent.unwrap_or(0), title, url) {
                        let _ = id; count += 1;
                    }
                }
                ui.set_status_text(SharedString::from(format!("Импортировано {count} ссылок из TXT")));
                refresh_ui(&ui, &st);
            }
        }
    }); }

    ui.run().unwrap();
}
