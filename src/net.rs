/// Fetch favicon for a URL and save to Data/favicons/ directory.
/// Returns the filename on success.
pub fn fetch_favicon(url: &str, favicons_dir: &std::path::Path) -> Option<String> {
    let domain = extract_domain(url)?;
    let safe = sanitize_domain(&domain);
    let filename = format!("{safe}.png");
    let dest = favicons_dir.join(&filename);

    // Already cached
    if dest.exists() && dest.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
        return Some(filename);
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124")
        .danger_accept_invalid_certs(true)
        .build().ok()?;

    // Strategy 1: favicon.ico at root
    let icon_url = format!("https://{domain}/favicon.ico");
    if let Some(bytes) = try_get(&client, &icon_url) {
        if is_valid_image(&bytes) {
            let _ = std::fs::create_dir_all(favicons_dir);
            let _ = std::fs::write(&dest, &bytes);
            return Some(filename);
        }
    }

    // Strategy 2: parse <link rel="icon"> from HTML
    if let Some(bytes) = try_get(&client, url) {
        let html = String::from_utf8_lossy(&bytes);
        if let Some(icon_href) = find_icon_link(&html, url) {
            if let Some(bytes2) = try_get(&client, &icon_href) {
                if is_valid_image(&bytes2) {
                    let _ = std::fs::create_dir_all(favicons_dir);
                    let _ = std::fs::write(&dest, &bytes2);
                    return Some(filename);
                }
            }
        }
    }

    // Strategy 3: DuckDuckGo favicon API
    let ddg = format!("https://icons.duckduckgo.com/ip3/{domain}.ico");
    if let Some(bytes) = try_get(&client, &ddg) {
        if is_valid_image(&bytes) {
            let _ = std::fs::create_dir_all(favicons_dir);
            let _ = std::fs::write(&dest, &bytes);
            return Some(filename);
        }
    }

    None
}

/// Check if a URL is reachable. Returns (ok, status_code_or_error).
pub fn check_url(url: &str) -> (bool, String) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124")
        .danger_accept_invalid_certs(true)
        .build() {
        Ok(c) => c,
        Err(e) => return (false, e.to_string()),
    };
    match client.head(url).send() {
        Ok(resp) => {
            let code = resp.status().as_u16();
            (code < 400, format!("{code}"))
        }
        Err(e) => {
            let msg = if e.is_timeout() { "Таймаут".to_string() }
                      else if e.is_connect() { "Нет соединения".to_string() }
                      else { e.to_string() };
            (false, msg)
        }
    }
}

fn try_get(client: &reqwest::blocking::Client, url: &str) -> Option<Vec<u8>> {
    client.get(url).send().ok()?.bytes().ok().map(|b| b.to_vec())
}

fn extract_domain(url: &str) -> Option<String> {
    let url = url.trim();
    let after_scheme = if let Some(p) = url.find("://") { &url[p+3..] } else { url };
    let host = after_scheme.split('/').next()?;
    let host = host.split(':').next()?;  // remove port
    let host = host.trim_start_matches("www.");
    if host.is_empty() { None } else { Some(host.to_lowercase()) }
}

fn sanitize_domain(domain: &str) -> String {
    domain.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

fn is_valid_image(bytes: &[u8]) -> bool {
    if bytes.len() < 4 { return false; }
    // PNG
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) { return true; }
    // ICO
    if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) { return true; }
    // JPEG
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) { return true; }
    // GIF
    if bytes.starts_with(b"GIF8") { return true; }
    // WebP
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" { return true; }
    // SVG (text)
    if let Ok(s) = std::str::from_utf8(&bytes[..bytes.len().min(100)]) {
        if s.contains("<svg") || s.contains("<?xml") { return true; }
    }
    false
}

fn find_icon_link(html: &str, base_url: &str) -> Option<String> {
    let lower = html.to_lowercase();
    // Find <link rel="icon" or rel="shortcut icon"
    let patterns = ["rel=\"icon\"", "rel=\"shortcut icon\"", "rel='icon'", "rel='shortcut icon'"];
    for pat in patterns {
        if let Some(pos) = lower.find(pat) {
            // Find the surrounding <link> tag
            let start = lower[..pos].rfind('<').unwrap_or(0);
            let end = lower[pos..].find('>').map(|e| pos + e + 1).unwrap_or(html.len());
            let tag = &html[start..end];
            // Extract href
            if let Some(href) = extract_attr(tag, "href") {
                return Some(resolve_url(&href, base_url));
            }
        }
    }
    None
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
    // Get base domain
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
