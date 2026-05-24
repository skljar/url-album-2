use std::io::Read;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Fetch favicon for a URL and save to Data/favicons/ directory.
/// Returns the filename on success.
pub fn fetch_favicon(url: &str, favicons_dir: &std::path::Path) -> Option<String> {
    let url = url.trim();
    // Skip non-web URLs (chrome://, about:, file://, etc.)
    if !url.starts_with("http://") && !url.starts_with("https://") { return None; }

    let domain = extract_domain(url)?;
    let safe = sanitize_domain(&domain);
    let filename = format!("{safe}.png");
    let dest = favicons_dir.join(&filename);

    // Cache hit — validate the cached file is a loadable image
    if dest.exists() {
        if let Ok(cached) = std::fs::read(&dest) {
            if is_valid_image(&cached) {
                return Some(filename);
            }
            // Corrupt/stale cache — delete and re-fetch
            let _ = std::fs::remove_file(&dest);
        }
    }

    let _ = std::fs::create_dir_all(favicons_dir);

    // Helper: prepare bytes for saving — extracts PNG from ICO, returns None for unloadable formats
    let try_save = |bytes: Vec<u8>| -> Option<()> {
        let saveable = prepare_image(bytes)?;
        std::fs::write(&dest, &saveable).ok()
    };

    // Strategy 1: /favicon.ico (HTTPS then HTTP fallback)
    for scheme in &["https", "http"] {
        if let Some(bytes) = try_get(&format!("{scheme}://{domain}/favicon.ico")) {
            if try_save(bytes).is_some() { return Some(filename); }
        }
    }

    // Strategy 2: all <link rel="icon"> in HTML (non-SVG links first, try each)
    let html_urls = [url.to_string(), format!("https://{domain}/"), format!("http://{domain}/")];
    'html: for fetch_url in &html_urls {
        if let Some(html_bytes) = try_get(fetch_url) {
            let html = String::from_utf8_lossy(&html_bytes);
            for icon_href in find_icon_links(&html, fetch_url) {
                if let Some(bytes) = try_get(&icon_href) {
                    if try_save(bytes).is_some() { return Some(filename); }
                }
            }
            if !html.is_empty() { break 'html; } // got HTML — no point trying root variants
        }
    }

    // Strategy 3: DuckDuckGo favicon service
    if let Some(bytes) = try_get(&format!("https://icons.duckduckgo.com/ip3/{domain}.ico")) {
        if try_save(bytes).is_some() { return Some(filename); }
    }

    // Strategy 4: Google S2 favicons (last resort)
    if let Some(bytes) = try_get(&format!("https://www.google.com/s2/favicons?domain={domain}&sz=32")) {
        // Google returns a tiny 1x1 placeholder (~68 bytes) when no favicon exists
        if bytes.len() > 68 {
            if try_save(bytes).is_some() { return Some(filename); }
        }
    }

    None
}

/// Decode any image format and re-encode as genuine PNG (so Slint can load by extension).
/// Returns None for SVG, too-small buffers, or unrecognised formats.
fn prepare_image(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if bytes.len() < 4 { return None; }
    // Reject SVG early — image crate won't handle it and it's not a raster format
    if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xm") { return None; }
    let img = image::load_from_memory(&bytes).ok()?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    if out.is_empty() { return None; }
    Some(out)
}

/// Check if a URL is reachable. Returns (ok, status_code_or_error).

pub fn check_url(url: &str) -> (bool, String) {
    match ureq::head(url)
        .timeout(std::time::Duration::from_secs(10))
        .set("User-Agent", UA)
        .call()
    {
        Ok(resp) => {
            let code = resp.status();
            (code < 400, format!("{code}"))
        }
        Err(ureq::Error::Status(code, _)) => {
            (code < 400, format!("{code}"))
        }
        Err(e) => {
            let msg = e.to_string();
            let brief = if msg.contains("timed out") || msg.contains("timeout") { "Таймаут".to_string() }
                        else if msg.contains("connect") { "Нет соединения".to_string() }
                        else { msg };
            (false, brief)
        }
    }
}

fn try_get(url: &str) -> Option<Vec<u8>> {
    match ureq::get(url)
        .timeout(std::time::Duration::from_secs(8))
        .set("User-Agent", UA)
        .call()
    {
        Ok(resp) => {
            let mut buf = Vec::new();
            match resp.into_reader().read_to_end(&mut buf) {
                Ok(_) => Some(buf),
                Err(e) => { favicon_log(&format!("read error {url}: {e}")); None }
            }
        }
        Err(e) => { favicon_log(&format!("get error {url}: {e}")); None }
    }
}

pub fn favicon_log(msg: &str) {
    let path = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.join("favicon_debug.log")));
    if let Some(path) = path {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

pub fn extract_domain(url: &str) -> Option<String> {
    let url = url.trim();
    let after_scheme = if let Some(p) = url.find("://") { &url[p+3..] } else { url };
    let host = after_scheme.split('/').next()?;
    let host = host.split(':').next()?;
    let host = host.trim_start_matches("www.");
    if host.is_empty() { None } else { Some(host.to_lowercase()) }
}

pub fn sanitize_domain(domain: &str) -> String {
    domain.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

/// All files saved by prepare_image() are genuine PNG. Any other magic → stale/corrupt → re-fetch.
fn is_valid_image(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47])
}

/// Returns all icon hrefs found in HTML, non-SVG links first.
fn find_icon_links(html: &str, base_url: &str) -> Vec<String> {
    let lower = html.to_lowercase();
    let patterns = ["rel=\"icon\"", "rel=\"shortcut icon\"", "rel='icon'", "rel='shortcut icon'",
                    "rel=\"apple-touch-icon\"", "rel='apple-touch-icon'"];
    let mut raster = Vec::new();
    let mut svg    = Vec::new();
    let mut search_from = 0;
    loop {
        // Find the earliest pattern occurrence after search_from
        let found = patterns.iter().filter_map(|pat| {
            lower[search_from..].find(pat).map(|p| p + search_from)
        }).min();
        let Some(pos) = found else { break };
        let start = lower[..pos].rfind('<').unwrap_or(0);
        let end = lower[pos..].find('>').map(|e| pos + e + 1).unwrap_or(html.len());
        let tag = &html[start..end];
        if let Some(href) = extract_attr(tag, "href") {
            let resolved = resolve_url(&href, base_url);
            let is_svg = href.to_lowercase().ends_with(".svg")
                || extract_attr(tag, "type").as_deref() == Some("image/svg+xml");
            if is_svg { svg.push(resolved); } else { raster.push(resolved); }
        }
        search_from = end.max(pos + 1);
    }
    // Return raster links first, SVG last (Slint can't render SVG)
    raster.extend(svg);
    raster
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    for q in ['"', '\''] {
        let needle = format!("{}={}", attr, q);
        if let Some(s) = lower.find(&needle) {
            let vs = s + needle.len();
            if let Some(e) = tag[vs..].find(q) {
                return Some(tag[vs..vs+e].to_string());
            }
        }
    }
    None
}

fn resolve_url(href: &str, base: &str) -> String {
    if href.starts_with("http") { return href.to_string(); }
    if href.starts_with("//") { return format!("https:{href}"); }
    if let Some(p) = base.find("://") {
        let after = &base[p+3..];
        let host_end = after.find('/').unwrap_or(after.len());
        let host = &after[..host_end];
        if href.starts_with('/') {
            return format!("https://{host}{href}");
        } else {
            let base_dir = base.rfind('/').map(|p| &base[..p]).unwrap_or(base);
            return format!("{base_dir}/{href}");
        }
    }
    href.to_string()
}
