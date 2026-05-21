mod db;

use db::Database;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

slint::include_modules!();

// ── App state ────────────────────────────────────────────────────────────────

struct State {
    db: Database,
    expanded: HashSet<i64>,
    active_folder: Option<i64>,
    selected_bookmark: Option<i64>,
    search_query: String,
}

impl State {
    fn new(db: Database) -> Self {
        State {
            db,
            expanded: HashSet::new(),
            active_folder: None,
            selected_bookmark: None,
            search_query: String::new(),
        }
    }

    fn build_folder_model(&self) -> ModelRc<FolderNode> {
        let all = self.db.get_all_folders().unwrap_or_default();
        let mut children: std::collections::HashMap<Option<i64>, Vec<&db::DbFolder>> = Default::default();
        for f in &all {
            children.entry(f.parent_id).or_default().push(f);
        }
        let mut result = Vec::new();
        self.walk_folders(None, 0, &children, &mut result);
        let vec: Vec<FolderNode> = result.into_iter().map(|(f, depth)| {
            FolderNode {
                id: f.id as i32,
                title: SharedString::from(f.title.as_str()),
                depth: depth as i32,
                expanded: self.expanded.contains(&f.id),
                has_children: self.db.has_children(f.id),
                selected: self.active_folder == Some(f.id),
            }
        }).collect();
        ModelRc::new(VecModel::from(vec))
    }

    fn walk_folders<'a>(
        &self,
        parent: Option<i64>,
        depth: usize,
        children: &std::collections::HashMap<Option<i64>, Vec<&'a db::DbFolder>>,
        out: &mut Vec<(&'a db::DbFolder, usize)>,
    ) {
        if let Some(kids) = children.get(&parent) {
            for f in kids {
                out.push((f, depth));
                if self.expanded.contains(&f.id) {
                    self.walk_folders(Some(f.id), depth + 1, children, out);
                }
            }
        }
    }

    fn build_bookmark_model(&self) -> ModelRc<BookmarkItem> {
        // Search mode
        if !self.search_query.is_empty() {
            let results = self.db.search(&self.search_query).unwrap_or_default();
            let vec: Vec<BookmarkItem> = results.into_iter().map(|b| BookmarkItem {
                id: b.id as i32,
                title: SharedString::from(b.title.as_str()),
                url: SharedString::from(b.url.as_deref().unwrap_or("")),
                selected: self.selected_bookmark == Some(b.id),
            }).collect();
            return ModelRc::new(VecModel::from(vec));
        }

        let bms = match self.active_folder {
            Some(id) => self.db.get_bookmarks(id).unwrap_or_default(),
            None => vec![],
        };
        let vec: Vec<BookmarkItem> = bms.into_iter().map(|b| BookmarkItem {
            id: b.id as i32,
            title: SharedString::from(b.title.as_str()),
            url: SharedString::from(b.url.as_deref().unwrap_or("")),
            selected: self.selected_bookmark == Some(b.id),
        }).collect();
        ModelRc::new(VecModel::from(vec))
    }

    fn status(&self) -> SharedString {
        if !self.search_query.is_empty() {
            let results = self.db.search(&self.search_query).unwrap_or_default();
            return SharedString::from(format!("Поиск: \"{}\" — найдено: {}", self.search_query, results.len()));
        }
        let (folders, bookmarks) = self.db.total_counts();
        match self.active_folder {
            Some(id) => {
                let cnt = self.db.bookmark_count(id);
                SharedString::from(format!("Ссылок в папке: {cnt}  |  Всего: папок {folders}, ссылок {bookmarks}"))
            }
            None => SharedString::from(format!("Папок: {folders}  |  Ссылок: {bookmarks}  |  Выберите папку")),
        }
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let db = Database::open_default().expect("Cannot open database");
    db.init_schema().expect("Cannot init schema");

    let state = Rc::new(RefCell::new(State::new(db)));
    let ui = MainWindow::new().unwrap();

    // Initial render
    refresh_ui(&ui, &state.borrow());

    // ── Folder clicked
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_folder_clicked(move |id| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            st.active_folder = Some(id as i64);
            st.selected_bookmark = None;
            st.expanded.insert(id as i64);
            refresh_ui(&ui, &st);
        });
    }

    // ── Folder toggle
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_folder_toggle(move |id| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            if st.expanded.contains(&(id as i64)) {
                st.expanded.remove(&(id as i64));
            } else {
                st.expanded.insert(id as i64);
            }
            ui.set_folders(st.build_folder_model());
        });
    }

    // ── Bookmark clicked (select)
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_bookmark_clicked(move |id| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            st.selected_bookmark = Some(id as i64);
            ui.set_bookmarks(st.build_bookmark_model());
        });
    }

    // ── Open URL
    ui.on_bookmark_open(|url| {
        open_url(url.as_str());
    });

    // ── Create folder
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_new_folder_confirmed(move |name| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            let parent = st.active_folder;
            if let Ok(new_id) = st.db.create_folder(parent, name.as_str()) {
                if let Some(p) = parent { st.expanded.insert(p); }
                st.active_folder = Some(new_id);
                st.expanded.insert(new_id);
            }
            refresh_ui(&ui, &st);
        });
    }

    // ── Create bookmark
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_new_bookmark_confirmed(move |title, url| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            if let Some(folder_id) = st.active_folder {
                let _ = st.db.create_bookmark(folder_id, title.as_str(), url.as_str());
                refresh_ui(&ui, &st);
            }
        });
    }

    // ── Delete selected
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_delete_selected(move || {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            if let Some(bm_id) = st.selected_bookmark {
                let _ = st.db.delete_bookmark(bm_id);
                st.selected_bookmark = None;
            } else if let Some(folder_id) = st.active_folder {
                let _ = st.db.delete_folder(folder_id);
                st.expanded.remove(&folder_id);
                st.active_folder = None;
            }
            refresh_ui(&ui, &st);
        });
    }

    // ── Search
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_search_changed(move |query| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            st.search_query = query.to_string();
            st.selected_bookmark = None;
            ui.set_bookmarks(st.build_bookmark_model());
            ui.set_status_text(st.status());
        });
    }

    // ── Import ua.dat
    {
        let s = state.clone();
        let ui_w = ui.as_weak();
        ui.on_import_uadat(move || {
            let ui = ui_w.unwrap();
            // Look for ua.dat next to exe first, then Desktop
            let candidates = vec![
                std::env::current_exe().unwrap_or_default().parent().unwrap_or(std::path::Path::new(".")).join("ua.dat"),
                dirs_path("Desktop").join("ua.dat"),
            ];
            for path in candidates {
                if path.exists() {
                    let mut st = s.borrow_mut();
                    match st.db.import_uadat(&path) {
                        Ok(n) => {
                            ui.set_status_text(SharedString::from(format!("Импортировано ссылок: {n}")));
                            refresh_ui(&ui, &st);
                        }
                        Err(e) => {
                            ui.set_status_text(SharedString::from(format!("Ошибка импорта: {e}")));
                        }
                    }
                    return;
                }
            }
            ui.set_status_text(SharedString::from("ua.dat не найден рядом с программой"));
        });
    }

    ui.run().unwrap();
}

fn refresh_ui(ui: &MainWindow, st: &State) {
    ui.set_folders(st.build_folder_model());
    ui.set_bookmarks(st.build_bookmark_model());
    ui.set_status_text(st.status());
}

fn open_url(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000)
        .spawn();
}

fn dirs_path(name: &str) -> std::path::PathBuf {
    std::env::var("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join(name))
        .unwrap_or_default()
}
