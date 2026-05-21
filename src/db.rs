use rusqlite::{Connection, Result, params};
use std::path::{Path, PathBuf};

pub struct DbFolder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
}

pub struct DbBookmark {
    pub id: i64,
    pub title: String,
    pub url: Option<String>,
    pub note: Option<String>,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open_at(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(Database { conn })
    }

    pub fn open_default() -> Result<Self> {
        Self::open_at(&Self::default_path())
    }

    pub fn default_path() -> PathBuf {
        std::env::current_exe().unwrap_or_default()
            .parent().unwrap_or(Path::new(".")).join("album.db")
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
        ")
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
            "SELECT id,title,url,note FROM nodes WHERE parent=?1 AND kind='bookmark' ORDER BY sort_idx,title")?;
        let mut result = Vec::new();
        for row in stmt.query_map(params![folder_id], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,Option<String>>(3)?)))? {
            let (id, title, url, note) = row?;
            result.push(DbBookmark { id, title, url, note });
        }
        Ok(result)
    }

    pub fn get_bookmark(&self, id: i64) -> Option<DbBookmark> {
        self.conn.query_row(
            "SELECT id,title,url,note FROM nodes WHERE id=?1",
            params![id], |r| Ok(DbBookmark {
                id: r.get(0)?, title: r.get(1)?, url: r.get(2)?, note: r.get(3)?
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
        self.conn.execute("UPDATE nodes SET note=?1 WHERE id=?2 AND kind='bookmark'",
            params![if filename.is_empty() { None } else { Some(filename) }, id])?;
        // Store favicon filename in a separate way — add favicon column if not exists
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN favicon TEXT;");
        self.conn.execute("UPDATE nodes SET favicon=?1 WHERE id=?2", params![filename, id])?;
        Ok(())
    }

    pub fn get_favicons(&self) -> std::collections::HashMap<i64, String> {
        let mut map = std::collections::HashMap::new();
        let _ = self.conn.execute_batch("ALTER TABLE nodes ADD COLUMN favicon TEXT;");
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT id, favicon FROM nodes WHERE kind='bookmark' AND favicon IS NOT NULL AND favicon != ''") {
            let _ = stmt.query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?)))
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

    pub fn delete_bookmark(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM nodes WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn bookmark_count(&self, folder_id: i64) -> i64 {
        self.conn.query_row("SELECT COUNT(*) FROM nodes WHERE parent=?1 AND kind='bookmark'", params![folder_id], |r| r.get(0)).unwrap_or(0)
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
            result.push(DbBookmark { id, title, url, note });
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
            result.push(DbBookmark { id, title, url, note });
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
            result.push(DbBookmark { id, title, url, note });
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

    pub fn import_chrome_json(&self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        let mut count = 0;
        self.parse_chrome_node(&content, None, &mut count)?;
        Ok(count)
    }

    fn parse_chrome_node(&self, json: &str, parent: Option<i64>, count: &mut usize) -> Result<()> {
        // Simple recursive JSON traversal without external JSON parser
        // Find all "children" arrays and "url"/"name" objects
        let mut pos = 0;
        while pos < json.len() {
            if let Some(idx) = json[pos..].find("\"type\": \"folder\"").or_else(|| json[pos..].find("\"type\":\"folder\"")) {
                let start = idx + pos;
                // Find enclosing object: scan backwards for opening {
                let obj_start = json[..start].rfind('{').unwrap_or(0);
                let name = extract_json_str(&json[obj_start..], "name").unwrap_or("Folder");
                let folder_id = self.create_folder(parent, &name)?;
                // Find children array
                if let Some(ch_idx) = json[obj_start..].find("\"children\"") {
                    let ch_start = obj_start + ch_idx;
                    if let Some(arr_start) = json[ch_start..].find('[') {
                        let arr_pos = ch_start + arr_start;
                        // Find matching ] - simplified: just recurse on the slice
                        let slice_end = find_matching_bracket(&json[arr_pos..]).unwrap_or(json.len() - arr_pos);
                        self.parse_chrome_node(&json[arr_pos..arr_pos+slice_end], Some(folder_id), count)?;
                    }
                }
                pos = start + 10;
            } else if let Some(idx) = json[pos..].find("\"type\": \"url\"").or_else(|| json[pos..].find("\"type\":\"url\"")) {
                let start = idx + pos;
                let obj_start = json[..start].rfind('{').unwrap_or(0);
                let url = extract_json_str(&json[obj_start..], "url").unwrap_or("");
                let name = extract_json_str(&json[obj_start..], "name").unwrap_or(url);
                if !url.is_empty() {
                    let p = parent.unwrap_or_else(|| self.create_folder(None, "Chrome Import").unwrap_or(1));
                    self.create_bookmark(p, &name, &url)?;
                    *count += 1;
                }
                pos = start + 10;
            } else {
                break;
            }
        }
        Ok(())
    }
}

fn extract_json_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{}\":", key);
    let pos = json.find(&needle)?;
    let after = json[pos + needle.len()..].trim_start();
    if !after.starts_with('"') { return None; }
    let content = &after[1..];
    let end = content.find('"')?;
    Some(&content[..end])
}

fn find_matching_bracket(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.chars().enumerate() {
        match c { '[' => depth += 1, ']' => { depth -= 1; if depth == 0 { return Some(i + 1); } } _ => {} }
    }
    None
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
