use rusqlite::{Connection, Result, params};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct DbFolder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
}

#[derive(Clone)]
pub struct DbBookmark {
    pub id: i64,
    pub title: String,
    pub url: Option<String>,
    pub note: Option<String>,
    pub created: Option<String>,
    pub thumb: Option<String>,
}

pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    pub fn open_at(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(Database { conn, path: path.clone() })
    }

    pub fn open_default() -> Result<Self> {
        Self::open_at(&Self::default_path())
    }

    pub fn default_path() -> PathBuf {
        std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(Path::new(".")).join("album.db")
    }

    pub fn path(&self) -> &PathBuf { &self.path }

    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub fn clone_empty(&self) -> Self {
        Self::open_default().unwrap_or_else(|_| panic!("DB error"))
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM nodes; DELETE FROM sqlite_sequence WHERE name='nodes'; VACUUM;")?;
        Ok(())
    }

    pub fn backup(&self, dest: &std::path::PathBuf) -> Result<()> {
        let mut dst = Connection::open(dest)?;
        let backup = rusqlite::backup::Backup::new(&self.conn, &mut dst)?;
        backup.run_to_completion(5, std::time::Duration::from_millis(250), None)?;
        Ok(())
    }

    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS nodes (
                id       INTEGER PRIMARY KEY AUTOINCREMENT,
                parent   INTEGER,
                kind     TEXT NOT NULL DEFAULT 'bookmark',
                title    TEXT NOT NULL,
                url      TEXT,
                note     TEXT,
                sort_idx INTEGER DEFAULT 0,
                created  TEXT DEFAULT (datetime('now'))
            );
        ")?;
        // Migrations — ignore errors if column already exists
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN favicon TEXT;");
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN visited TEXT;");
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN thumb TEXT;");
        Ok(())
    }

    // ── Folders ──────────────────────────────────────────────────────────────

    pub fn get_all_folders(&self) -> Result<Vec<DbFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, parent, title FROM nodes WHERE kind='folder' ORDER BY parent, sort_idx, title")?;
        let mut result = Vec::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,Option<i64>>(1)?, r.get::<_,String>(2)?)))? {
            let (id, parent_id, title) = row?;
            result.push(DbFolder { id, parent_id, title });
        }
        Ok(result)
    }

    pub fn create_folder(&self, parent_id: Option<i64>, title: &str) -> Result<i64> {
        self.conn.execute("INSERT INTO nodes (parent,kind,title) VALUES (?1,'folder',?2)", params![parent_id, title])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_node(&self, id: i64, title: &str) -> Result<()> {
        self.conn.execute("UPDATE nodes SET title=?1 WHERE id=?2", params![title, id])?;
        Ok(())
    }

    pub fn delete_folder(&self, id: i64) -> Result<()> {
        self.conn.execute_batch(&format!("
            WITH RECURSIVE sub(id) AS (
                SELECT {id} UNION ALL SELECT n.id FROM nodes n JOIN sub s ON n.parent=s.id
            ) DELETE FROM nodes WHERE id IN (SELECT id FROM sub);
        "))
    }

    pub fn get_folder_title(&self, id: i64) -> Option<String> {
        self.conn.query_row("SELECT title FROM nodes WHERE id=?1", params![id], |r| r.get(0)).ok()
    }

    pub fn get_breadcrumb(&self, folder_id: i64) -> Result<String> {
        let mut path = Vec::new();
        let mut current = folder_id;
        loop {
            if let Ok((title, parent)) = self.conn.query_row(
                "SELECT title, parent FROM nodes WHERE id=?1", params![current],
                |r| Ok((r.get::<_,String>(0)?, r.get::<_,Option<i64>>(1)?))) {
                path.push(title);
                match parent { Some(p) if p > 0 => current = p, _ => break }
            } else { break; }
            if path.len() > 10 { break; } // safety
        }
        path.reverse();
        Ok(path.join(" › "))
    }

    // ── Bookmarks ─────────────────────────────────────────────────────────────

    pub fn get_bookmarks(&self, folder_id: i64) -> Result<Vec<DbBookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,title,url,note,created,thumb FROM nodes WHERE parent=?1 AND kind='bookmark' ORDER BY sort_idx,title")?;
        let mut result = Vec::new();
        for row in stmt.query_map(params![folder_id], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,Option<String>>(3)?, r.get::<_,Option<String>>(4)?, r.get::<_,Option<String>>(5)?)))? {
            let (id, title, url, note, created, thumb) = row?;
            result.push(DbBookmark { id, title, url, note, created, thumb });
        }
        Ok(result)
    }

    pub fn get_bookmarks_recursive(&self, folder_id: i64) -> Vec<DbBookmark> {
        let mut result = Vec::new();
        self.collect_bookmarks_recursive(folder_id, &mut result);
        result
    }

    fn collect_bookmarks_recursive(&self, folder_id: i64, out: &mut Vec<DbBookmark>) {
        if let Ok(bms) = self.get_bookmarks(folder_id) { out.extend(bms); }
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT id FROM nodes WHERE parent=?1 AND kind='folder'") {
            let ids: Vec<i64> = stmt.query_map(params![folder_id], |r| r.get(0))
                .map(|rows| rows.flatten().collect()).unwrap_or_default();
            for sub_id in ids { self.collect_bookmarks_recursive(sub_id, out); }
        }
    }

    pub fn get_bookmark(&self, id: i64) -> Option<DbBookmark> {
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN thumb TEXT;");
        self.conn.query_row(
            "SELECT id,title,url,note,created,thumb FROM nodes WHERE id=?1",
            params![id], |r| Ok(DbBookmark {
                id: r.get(0)?, title: r.get(1)?, url: r.get(2)?, note: r.get(3)?,
                created: r.get(4)?, thumb: r.get(5)?
            })).ok()
    }

    pub fn get_bookmark_title(&self, id: i64) -> Option<String> {
        self.conn.query_row("SELECT title FROM nodes WHERE id=?1", params![id], |r| r.get(0)).ok()
    }

    pub fn create_bookmark(&self, parent_id: i64, title: &str, url: &str) -> Result<i64> {
        let t = if title.is_empty() { url } else { title };
        self.conn.execute("INSERT INTO nodes (parent,kind,title,url) VALUES (?1,'bookmark',?2,?3)", params![parent_id, t, url])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_favicon(&self, id: i64, filename: &str) -> Result<()> {
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN favicon TEXT;");
        self.conn.execute("UPDATE nodes SET favicon=?1 WHERE id=?2",
            params![if filename.is_empty() { None::<&str> } else { Some(filename) }, id])?;
        Ok(())
    }

    pub fn get_favicons(&self) -> std::collections::HashMap<i64, String> {
        let mut map = std::collections::HashMap::new();
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT id, favicon FROM nodes WHERE kind='bookmark' AND favicon IS NOT NULL AND favicon != ''") {
            let _ = stmt.query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?)))
                .map(|rows| for row in rows.flatten() { map.insert(row.0, row.1); });
        }
        map
    }

    // Single query: bookmark count per folder (direct children only)
    pub fn get_all_bookmark_counts(&self) -> std::collections::HashMap<i64, i64> {
        let mut map = std::collections::HashMap::new();
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT parent, COUNT(*) FROM nodes WHERE kind='bookmark' AND parent IS NOT NULL GROUP BY parent") {
            let _ = stmt.query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,i64>(1)?)))
                .map(|rows| for row in rows.flatten() { map.insert(row.0, row.1); });
        }
        map
    }

    pub fn update_bookmark(&self, id: i64, title: &str, url: &str, note: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET title=?1, url=?2, note=?3 WHERE id=?4",
            params![title, url, if note.is_empty() { None } else { Some(note) }, id])?;
        Ok(())
    }

    pub fn move_node(&self, id: i64, new_parent: i64) -> Result<()> {
        self.conn.execute("UPDATE nodes SET parent=?1 WHERE id=?2", params![new_parent, id])?;
        Ok(())
    }

    pub fn move_item_relative(&self, id: i64, delta: i64) -> Result<()> {
        // Get kind and parent of the item
        let (kind, parent): (String, Option<i64>) = self.conn.query_row(
            "SELECT kind, parent FROM nodes WHERE id=?1", params![id],
            |r| Ok((r.get(0)?, r.get(1)?))).unwrap_or_default();

        // Get all siblings of the same kind in display order
        let mut siblings: Vec<i64> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM nodes WHERE parent IS ?1 AND kind=?2 ORDER BY sort_idx, title")?;
            let rows = stmt.query_map(params![parent, &kind], |r| r.get::<_, i64>(0))?;
            for row in rows.flatten() { siblings.push(row); }
        }

        let pos = match siblings.iter().position(|&x| x == id) {
            Some(p) => p,
            None => return Ok(()),
        };
        let new_pos = pos as i64 + delta;
        if new_pos < 0 || new_pos >= siblings.len() as i64 { return Ok(()); }

        // Swap and write dense sort_idx values so order is stable
        siblings.swap(pos, new_pos as usize);
        for (i, &sid) in siblings.iter().enumerate() {
            self.conn.execute("UPDATE nodes SET sort_idx=?1 WHERE id=?2", params![i as i64, sid])?;
        }
        Ok(())
    }

    pub fn sort_folder(&self, folder_id: i64) -> Result<()> {
        self.conn.execute_batch(&format!(
            "UPDATE nodes SET sort_idx = (SELECT COUNT(*) FROM nodes n2
             WHERE n2.parent={folder_id} AND n2.kind=nodes.kind AND LOWER(n2.title) < LOWER(nodes.title))
             WHERE parent={folder_id}"))?;
        Ok(())
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM nodes WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn total_counts(&self) -> (i64, i64) {
        let f = self.conn.query_row("SELECT COUNT(*) FROM nodes WHERE kind='folder'", [], |r| r.get(0)).unwrap_or(0);
        let b = self.conn.query_row("SELECT COUNT(*) FROM nodes WHERE kind='bookmark'", [], |r| r.get(0)).unwrap_or(0);
        (f, b)
    }

    // ── Search ────────────────────────────────────────────────────────────────

    pub fn get_all_bookmarks(&self) -> Result<Vec<DbBookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, url, note FROM nodes WHERE kind='bookmark' ORDER BY title")?;
        let mut result = Vec::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,Option<String>>(3)?)))? {
            let (id, title, url, note) = row?;
            result.push(DbBookmark { id, title, url, note, created: None, thumb: None });
        }
        Ok(result)
    }

    pub fn find_duplicates(&self) -> Result<Vec<DbBookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, url, note FROM nodes WHERE kind='bookmark'
             AND url IN (SELECT url FROM nodes WHERE kind='bookmark' AND url IS NOT NULL GROUP BY url HAVING COUNT(*) > 1)
             ORDER BY url, title")?;
        let mut result = Vec::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,Option<String>>(3)?)))? {
            let (id, title, url, note) = row?;
            result.push(DbBookmark { id, title, url, note, created: None, thumb: None });
        }
        Ok(result)
    }

    pub fn search(&self, query: &str) -> Result<Vec<DbBookmark>> {
        let q = format!("%{}%", query.to_lowercase());
        let mut stmt = self.conn.prepare(
            "SELECT id,title,url,note FROM nodes WHERE kind='bookmark'
             AND (LOWER(title) LIKE ?1 OR LOWER(COALESCE(url,'')) LIKE ?1 OR LOWER(COALESCE(note,'')) LIKE ?1)
             ORDER BY title")?;
        let mut result = Vec::new();
        for row in stmt.query_map(params![q], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,Option<String>>(3)?)))? {
            let (id, title, url, note) = row?;
            result.push(DbBookmark { id, title, url, note, created: None, thumb: None });
        }
        Ok(result)
    }

    // ── Export ────────────────────────────────────────────────────────────────

    pub fn export_html(&self, path: &PathBuf) -> Result<usize> {
        struct Row { kind: String, title: String, url: Option<String>, depth: i64 }

        let mut stmt = self.conn.prepare("
            WITH RECURSIVE tree(id, parent, kind, title, url, depth) AS (
                SELECT id,parent,kind,title,url,0 FROM nodes WHERE parent IS NULL OR parent=0
                UNION ALL SELECT n.id,n.parent,n.kind,n.title,n.url,t.depth+1 FROM nodes n JOIN tree t ON n.parent=t.id
            ) SELECT kind,title,url,depth FROM tree ORDER BY depth,title
        ")?;

        let mut rows = Vec::new();
        for r in stmt.query_map([], |r| Ok(Row { kind: r.get(0)?, title: r.get(1)?, url: r.get(2)?, depth: r.get(3)? }))? {
            rows.push(r?);
        }

        let mut html = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n");
        let mut count = 0;
        for row in &rows {
            let ind = "    ".repeat(row.depth as usize);
            if row.kind == "folder" {
                html.push_str(&format!("{ind}<DT><H3>{}</H3>\n{ind}<DL><p>\n", esc(&row.title)));
            } else if let Some(u) = &row.url {
                html.push_str(&format!("{ind}<DT><A HREF=\"{u}\">{}</A>\n", esc(&row.title)));
                count += 1;
            }
        }
        html.push_str("</DL><p>\n");
        std::fs::write(path, html.as_bytes()).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        Ok(count)
    }

    pub fn export_txt(&self, path: &PathBuf) -> Result<usize> {
        let mut stmt = self.conn.prepare("SELECT title,url FROM nodes WHERE kind='bookmark' ORDER BY title")?;
        let mut lines = Vec::new();
        for r in stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,Option<String>>(1)?)))? {
            let (title, url) = r?;
            if let Some(u) = url { lines.push(format!("{u}\t{title}\n")); }
        }
        let count = lines.len();
        std::fs::write(path, lines.concat().as_bytes()).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        Ok(count)
    }

    // ── Import ua.dat ─────────────────────────────────────────────────────────
    // Format: leading tabs = depth, fields tab-separated:
    // title \t url \t image \t note \t created \t visited \t 0
    // If url == "#" → folder node

    pub fn import_uadat(&self, path: &Path) -> Result<usize> {
        let bytes = std::fs::read(path).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        let (text, _, _) = encoding_rs::WINDOWS_1251.decode(&bytes);

        let mut count = 0;
        // folder_stack[depth] = folder_id at that depth
        let mut folder_stack: Vec<Option<i64>> = vec![None]; // depth 0 = root (no parent)

        for line in text.lines() {
            if line.trim().is_empty() { continue; }

            // Count leading tabs = depth of this item
            let depth = line.chars().take_while(|&c| c == '\t').count();
            let content = &line[depth..]; // strip leading tabs

            // Split by tab
            let fields: Vec<&str> = content.splitn(7, '\t').collect();
            if fields.is_empty() { continue; }

            let title = fields[0].trim();
            let url = fields.get(1).map(|s| s.trim()).unwrap_or("");
            let note = fields.get(3).map(|s| s.trim()).unwrap_or("");

            if title.is_empty() { continue; }

            // Ensure stack is large enough
            while folder_stack.len() <= depth { folder_stack.push(None); }

            // Parent is the folder at depth-1
            let parent = if depth == 0 { None } else { folder_stack[depth - 1] };

            if url == "#" {
                // It's a folder
                let folder_id = self.create_folder(parent, title)?;
                // Store at this depth level
                if folder_stack.len() <= depth { folder_stack.push(Some(folder_id)); }
                else { folder_stack[depth] = Some(folder_id); }
                // Clear deeper levels
                for i in (depth + 1)..folder_stack.len() { folder_stack[i] = None; }
            } else if !url.is_empty() && url != "#" {
                // It's a bookmark
                if let Some(parent_id) = parent {
                    let note_str = if note.is_empty() { "" } else { note };
                    self.conn.execute(
                        "INSERT INTO nodes (parent, kind, title, url, note) VALUES (?1,'bookmark',?2,?3,?4)",
                        rusqlite::params![parent_id, title, url, if note_str.is_empty() { None } else { Some(note_str) }])?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    // ── Import HTML (Netscape) ────────────────────────────────────────────────

    pub fn import_html(&self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        let mut count = 0;
        let mut stack: Vec<i64> = vec![];
        let mut current: Option<i64> = None;
        for line in content.lines() {
            let lo = line.to_lowercase();
            if lo.contains("<h3") {
                let title = tag_content(line, "h3");
                if !title.is_empty() {
                    let id = self.create_folder(current, &unesc(&title))?;
                    stack.push(id); current = Some(id);
                }
            } else if lo.contains("<a ") && lo.contains("href=") {
                let url = attr_val(line, "href");
                let title = tag_content(line, "a");
                if !url.is_empty() {
                    let parent = current.unwrap_or_else(|| self.create_folder(None, "Импорт").unwrap_or(1));
                    self.create_bookmark(parent, &unesc(&title), &url)?; count += 1;
                }
            } else if lo.trim() == "</dl>" || lo.trim() == "</dl><p>" {
                stack.pop(); current = stack.last().copied();
            }
        }
        Ok(count)
    }

    // ── Import from browser ───────────────────────────────────────────────────

    pub fn import_chrome_json_named(&self, path: &Path, browser_name: &str) -> Result<usize> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        let json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        let root_id = self.create_folder(None, browser_name)?;
        let mut count = 0;
        if let Some(roots) = json["roots"].as_object() {
            for (_key, node) in roots {
                self.import_chrome_node(node, root_id, &mut count)?;
            }
        }
        Ok(count)
    }

    fn import_chrome_node(&self, node: &serde_json::Value, parent: i64, count: &mut usize) -> Result<()> {
        match node["type"].as_str().unwrap_or("") {
            "folder" => {
                let name = node["name"].as_str().unwrap_or("Папка");
                let folder_id = self.create_folder(Some(parent), name)?;
                if let Some(children) = node["children"].as_array() {
                    for child in children {
                        self.import_chrome_node(child, folder_id, count)?;
                    }
                }
            }
            "url" => {
                if let Some(url) = node["url"].as_str() {
                    if !url.is_empty() {
                        let name = node["name"].as_str().unwrap_or(url);
                        self.create_bookmark(parent, name, url)?;
                        *count += 1;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn import_firefox(&self, places_path: &Path, browser_name: &str) -> Result<usize> {
        let tmp     = std::env::temp_dir().join("ua3_ff_tmp.sqlite");
        let tmp_wal = std::env::temp_dir().join("ua3_ff_tmp.sqlite-wal");
        let tmp_shm = std::env::temp_dir().join("ua3_ff_tmp.sqlite-shm");
        std::fs::copy(places_path, &tmp)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        for (ext, dst) in &[("-wal", &tmp_wal), ("-shm", &tmp_shm)] {
            let src = format!("{}{}", places_path.to_string_lossy(), ext);
            if std::path::Path::new(&src).exists() { let _ = std::fs::copy(&src, dst); }
        }
        let ff = rusqlite::Connection::open_with_flags(&tmp, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let root_ff_id: i64 = ff.query_row(
            "SELECT id FROM moz_bookmarks WHERE parent IS NULL LIMIT 1", [], |r| r.get(0),
        ).unwrap_or(1);
        struct FfNode { id: i64, parent: i64, bk_type: i64, title: Option<String>, fk: Option<i64> }
        let mut stmt = ff.prepare(
            "SELECT id, COALESCE(parent,0), type, title, fk FROM moz_bookmarks ORDER BY parent, position",
        )?;
        let ff_nodes: Vec<FfNode> = stmt.query_map([], |r| Ok(FfNode {
            id: r.get(0)?, parent: r.get(1)?, bk_type: r.get(2)?,
            title: r.get(3)?, fk: r.get(4)?,
        }))?.filter_map(|r| r.ok()).collect();
        let mut pstmt = ff.prepare("SELECT id, url FROM moz_places")?;
        let places: std::collections::HashMap<i64, String> =
            pstmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
        drop(stmt); drop(pstmt); drop(ff);
        let _ = (std::fs::remove_file(&tmp), std::fs::remove_file(&tmp_wal), std::fs::remove_file(&tmp_shm));
        let db_root = self.create_folder(None, browser_name)?;
        let mut id_map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        id_map.insert(root_ff_id, db_root);
        let mut count = 0;
        for node in &ff_nodes {
            if node.id == root_ff_id { continue; }
            let Some(&parent_db) = id_map.get(&node.parent) else { continue };
            match node.bk_type {
                2 => {
                    let title = node.title.as_deref().filter(|t| !t.is_empty()).unwrap_or("Закладки");
                    let fid = self.create_folder(Some(parent_db), title)?;
                    id_map.insert(node.id, fid);
                }
                1 => if let Some(fk) = node.fk {
                    if let Some(url) = places.get(&fk) {
                        if !url.starts_with("place:") {
                            let title = node.title.as_deref().filter(|t| !t.is_empty()).unwrap_or(url.as_str());
                            self.create_bookmark(parent_db, title, url)?;
                            count += 1;
                        }
                    }
                },
                _ => {}
            }
        }
        Ok(count)
    }

}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn unesc(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
}

fn tag_content(line: &str, tag: &str) -> String {
    let lo = line.to_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    if let (Some(s), Some(e)) = (lo.find(&open), lo.find(&close)) {
        if let Some(gt) = lo[s..].find('>') {
            let cs = s + gt + 1;
            if cs < e { return line[cs..e].to_string(); }
        }
    }
    String::new()
}

fn attr_val(line: &str, attr: &str) -> String {
    let lo = line.to_lowercase();
    for q in ['"', '\''] {
        let needle = format!("{}={}", attr, q);
        if let Some(s) = lo.find(&needle) {
            let vs = s + needle.len();
            if let Some(e) = line[vs..].find(q) { return line[vs..vs+e].to_string(); }
        }
    }
    String::new()
}
