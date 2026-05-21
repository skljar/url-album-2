use rusqlite::{Connection, Result, params};
use std::path::PathBuf;

pub struct DbFolder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
}

pub struct DbBookmark {
    pub id: i64,
    pub title: String,
    pub url: Option<String>,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open_at(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA foreign_keys=ON;
        ")?;
        Ok(Database { conn })
    }

    pub fn open_default() -> Result<Self> {
        let path = Self::default_path();
        Self::open_at(&path)
    }

    pub fn default_path() -> PathBuf {
        std::env::current_exe()
            .unwrap_or_default()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("album.db")
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
        Ok(())
    }

    // ── Folders ─────────────────────────────────────────────────────────────

    pub fn get_all_folders(&self) -> Result<Vec<DbFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, parent, title FROM nodes WHERE kind='folder' ORDER BY parent, sort_idx, title"
        )?;
        let rows = stmt.query_map([], |r| Ok(DbFolder {
            id: r.get(0)?,
            parent_id: r.get(1)?,
            title: r.get(2)?,
        }))?;
        rows.collect()
    }

    pub fn create_folder(&self, parent_id: Option<i64>, title: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO nodes (parent, kind, title) VALUES (?1, 'folder', ?2)",
            params![parent_id, title],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_node(&self, id: i64, title: &str) -> Result<()> {
        self.conn.execute("UPDATE nodes SET title=?1 WHERE id=?2", params![title, id])?;
        Ok(())
    }

    pub fn delete_folder(&self, id: i64) -> Result<()> {
        // Recursive delete via CTE
        self.conn.execute_batch(&format!("
            WITH RECURSIVE sub(id) AS (
                SELECT {id}
                UNION ALL
                SELECT n.id FROM nodes n JOIN sub s ON n.parent=s.id
            )
            DELETE FROM nodes WHERE id IN (SELECT id FROM sub);
        "))?;
        Ok(())
    }

    pub fn has_children(&self, id: i64) -> bool {
        self.conn
            .query_row("SELECT COUNT(*) FROM nodes WHERE parent=?1 AND kind='folder'", params![id], |r| r.get::<_, i64>(0))
            .unwrap_or(0) > 0
    }

    // ── Bookmarks ────────────────────────────────────────────────────────────

    pub fn get_bookmarks(&self, folder_id: i64) -> Result<Vec<DbBookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, url FROM nodes WHERE parent=?1 AND kind='bookmark' ORDER BY sort_idx, title"
        )?;
        let rows = stmt.query_map(params![folder_id], |r| Ok(DbBookmark {
            id: r.get(0)?,
            title: r.get(1)?,
            url: r.get(2)?,
        }))?;
        rows.collect()
    }

    pub fn create_bookmark(&self, parent_id: i64, title: &str, url: &str) -> Result<i64> {
        let t = if title.is_empty() { url } else { title };
        self.conn.execute(
            "INSERT INTO nodes (parent, kind, title, url) VALUES (?1, 'bookmark', ?2, ?3)",
            params![parent_id, t, url],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_bookmark(&self, id: i64, title: &str, url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET title=?1, url=?2 WHERE id=?3",
            params![title, url, id],
        )?;
        Ok(())
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM nodes WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn get_node_kind(&self, id: i64) -> Option<String> {
        self.conn
            .query_row("SELECT kind FROM nodes WHERE id=?1", params![id], |r| r.get(0))
            .ok()
    }

    pub fn bookmark_count(&self, folder_id: i64) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM nodes WHERE parent=?1 AND kind='bookmark'", params![folder_id], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn total_counts(&self) -> (i64, i64) {
        let folders = self.conn
            .query_row("SELECT COUNT(*) FROM nodes WHERE kind='folder'", [], |r| r.get(0))
            .unwrap_or(0);
        let bookmarks = self.conn
            .query_row("SELECT COUNT(*) FROM nodes WHERE kind='bookmark'", [], |r| r.get(0))
            .unwrap_or(0);
        (folders, bookmarks)
    }

    // ── Search ───────────────────────────────────────────────────────────────

    pub fn search(&self, query: &str) -> Result<Vec<DbBookmark>> {
        let q = format!("%{}%", query.to_lowercase());
        let mut stmt = self.conn.prepare(
            "SELECT id, title, url FROM nodes WHERE kind='bookmark'
             AND (LOWER(title) LIKE ?1 OR LOWER(url) LIKE ?1)
             ORDER BY title"
        )?;
        let rows = stmt.query_map(params![q], |r| Ok(DbBookmark {
            id: r.get(0)?,
            title: r.get(1)?,
            url: r.get(2)?,
        }))?;
        rows.collect()
    }

    // ── Import from ua.dat ───────────────────────────────────────────────────

    pub fn import_uadat(&self, path: &std::path::Path) -> Result<usize> {
        use std::io::BufRead;

        let file = std::fs::File::open(path).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;

        // ua.dat is Windows-1251 encoded
        let bytes = std::fs::read(path).unwrap_or_default();
        let (text, _, _) = encoding_rs::WINDOWS_1251.decode(&bytes);

        let mut count = 0;
        let mut folder_stack: Vec<i64> = vec![]; // root = no parent
        let mut current_parent: Option<i64> = None;

        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                // Folder
                let name = &line[1..line.len()-1];
                let parent = current_parent;
                let id = self.create_folder(parent, name)?;
                folder_stack.push(id);
                current_parent = Some(id);
            } else if line == "{" {
                // enter subfolder - already handled
            } else if line == "}" {
                folder_stack.pop();
                current_parent = folder_stack.last().copied();
            } else if line.contains('\t') {
                let parts: Vec<&str> = line.splitn(2, '\t').collect();
                if parts.len() == 2 {
                    let url = parts[0].trim();
                    let title = parts[1].trim();
                    if !url.is_empty() && current_parent.is_some() {
                        self.create_bookmark(current_parent.unwrap(), title, url)?;
                        count += 1;
                    }
                }
            } else if line.starts_with("http") || line.starts_with("ftp") {
                if current_parent.is_some() {
                    self.create_bookmark(current_parent.unwrap(), line, line)?;
                    count += 1;
                }
            }
        }
        let _ = file;
        Ok(count)
    }
}
