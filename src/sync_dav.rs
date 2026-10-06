//! Small WebDAV transport. Credentials are passed to curl over stdin, never argv.
use gtk::glib;
use quick_xml::{Reader, events::Event};
use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Stdio},
};

#[derive(Clone)]
pub struct Dav {
    pub url: String,
    pub username: String,
    pub password: String,
    pub lock_writes: bool,
}
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub lock_token: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Remote {
    pub modified: Option<std::time::SystemTime>,
    pub etag: String,
}
#[derive(Default)]
struct Item {
    modified: String,
    href: String,
    etag: String,
    collection: bool,
    status: Vec<String>,
}

fn parse_modified(value: &str) -> Option<std::time::SystemTime> {
    // WebDAV getlastmodified uses the HTTP IMF-fixdate format, in GMT.
    let parts: Vec<_> = value.split_whitespace().collect();
    if parts.len() != 6 || parts[5] != "GMT" {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == parts[2])?
        + 1;
    let time: Vec<_> = parts[4].split(':').collect();
    if time.len() != 3 {
        return None;
    }
    let date = glib::DateTime::from_utc(
        parts[3].parse().ok()?,
        month as i32,
        parts[1].parse().ok()?,
        time[0].parse().ok()?,
        time[1].parse().ok()?,
        time[2].parse().ok()?,
    )
    .ok()?;
    let seconds = date.to_unix();
    if seconds >= 0 {
        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds as u64))
    } else {
        std::time::UNIX_EPOCH.checked_sub(std::time::Duration::from_secs(seconds.unsigned_abs()))
    }
}

pub fn normalize_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let value = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let uri = glib::Uri::parse(&value, glib::UriFlags::NONE)
        .map_err(|_| "Enter a valid WebDAV URL".to_string())?;
    if !matches!(uri.scheme().as_str(), "http" | "https")
        || uri.host().is_none()
        || uri.userinfo().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return Err(
            "Use an HTTP or HTTPS URL, without embedded credentials, query, or fragment".into(),
        );
    }
    Ok(format!("{}/", value.trim_end_matches('/')))
}
pub fn hash(bytes: &[u8]) -> String {
    let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256).unwrap();
    checksum.update(bytes);
    checksum.string().unwrap().to_string()
}
pub fn encode(path: &str) -> String {
    path.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
fn quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
impl Dav {
    pub fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<Response, String> {
        // A private scratch directory also isolates concurrent requests.
        let scratch = std::env::temp_dir().join(format!(
            "hematite-dav-{}-{}",
            std::process::id(),
            glib::uuid_string_random()
        ));
        std::fs::create_dir(&scratch).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let result = (|| {
            let mut config = format!(
                "url = \"{}\"\nuser = \"{}\"\n",
                quoted(&format!("{}{}", self.url, encode(path))),
                quoted(&format!("{}:{}", self.username, self.password))
            );
            let mut command = Command::new("curl");
            command
                .args([
                    "--disable",
                    "--silent",
                    "--show-error",
                    "--connect-timeout",
                    "10",
                    "--max-time",
                    "60",
                    "--max-filesize",
                    "268435456",
                    "--request",
                    method,
                    "--dump-header",
                ])
                .arg(scratch.join("headers"))
                .args(["--write-out", "\n%{http_code}", "--config", "-"]);
            for (name, value) in headers {
                command.arg("--header").arg(format!("{name}: {value}"));
            }
            if let Some(body) = body {
                let file = scratch.join("body");
                std::fs::write(&file, body).map_err(|e| e.to_string())?;
                config.push_str(&format!(
                    "data-binary = \"@{}\"\n",
                    quoted(&file.to_string_lossy())
                ));
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("Could not start curl: {e}"))?;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(config.as_bytes())
                .map_err(|e| e.to_string())?;
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "Could not reach WebDAV server: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let split = output
                .stdout
                .iter()
                .rposition(|byte| *byte == b'\n')
                .ok_or("Invalid HTTP response")?;
            let status = String::from_utf8_lossy(&output.stdout[split + 1..])
                .parse::<u16>()
                .map_err(|_| "Invalid HTTP status")?;
            let header_text =
                std::fs::read_to_string(scratch.join("headers")).map_err(|e| e.to_string())?;
            let etag = header_text
                .lines()
                .filter_map(|line| line.split_once(':'))
                .filter(|(key, _)| key.eq_ignore_ascii_case("etag"))
                .map(|(_, value)| value.trim().to_string())
                .last();
            let lock_token = header_text
                .lines()
                .filter_map(|line| line.split_once(':'))
                .filter(|(key, _)| key.eq_ignore_ascii_case("lock-token"))
                .map(|(_, value)| value.trim().to_string())
                .last();
            Ok(Response {
                status,
                body: output.stdout[..split].to_vec(),
                etag,
                lock_token,
            })
        })();
        let _ = std::fs::remove_dir_all(scratch);
        result
    }
    pub fn checked(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<Response, String> {
        let response = self.request(method, path, headers, body)?;
        if !(200..300).contains(&response.status) {
            return Err(match response.status {
                401 | 403 => {
                    "WebDAV login or access was denied. Check your credentials and server URL."
                        .into()
                }
                412 => {
                    format!("Remote file changed during sync: {path}. It will be checked again.")
                }
                code => format!("WebDAV {method} failed with HTTP {code} for {path}"),
            });
        }
        Ok(response)
    }
    pub fn get(&self, path: &str) -> Result<(Vec<u8>, String), String> {
        let response = self.checked("GET", path, &[], None)?;
        let etag = response
            .etag
            .filter(|tag| tag.starts_with('"') && tag.ends_with('"'))
            .ok_or("The server must provide strong ETags for safe syncing")?;
        Ok((response.body, etag))
    }
    pub fn put(&self, path: &str, bytes: &[u8], etag: Option<&str>) -> Result<String, String> {
        let mut parent = String::new();
        let pieces: Vec<_> = path.split('/').collect();
        for piece in &pieces[..pieces.len() - 1] {
            parent.push_str(piece);
            parent.push('/');
            let response = self.request("MKCOL", &parent, &[], None)?;
            if !(200..300).contains(&response.status) && response.status != 405 {
                return Err(format!(
                    "Could not create remote folder: HTTP {}",
                    response.status
                ));
            }
        }
        let condition = if let Some(etag) = etag {
            ("If-Match", etag)
        } else {
            ("If-None-Match", "*")
        };
        if self.lock_writes {
            self.locked_write("PUT", path, Some(bytes), etag)?;
        } else {
            self.checked("PUT", path, &[condition], Some(bytes))?;
        }
        let (actual, tag) = self.get(path)?;
        if actual != bytes {
            return Err(format!("Remote verification failed for {path}"));
        }
        Ok(tag)
    }
    fn locked_write(
        &self,
        method: &str,
        path: &str,
        bytes: Option<&[u8]>,
        expected: Option<&str>,
    ) -> Result<(), String> {
        let lock = self.checked("LOCK", path, &[("Content-Type", "application/xml"), ("Timeout", "Second-300"), ("Depth", "0")], Some(b"<d:lockinfo xmlns:d=\"DAV:\"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockinfo>"))?;
        let token = lock
            .lock_token
            .ok_or("Server did not provide a lock token")?;
        let condition = format!("({token})");
        let operation = (|| {
            let (_, actual) = self.get(path)?;
            if expected.is_some_and(|expected| lock.status != 200 || expected != actual)
                || (expected.is_none() && lock.status != 201)
            {
                return Err(format!(
                    "Remote file changed during sync: {path}. Retrying safely."
                ));
            }
            self.checked(method, path, &[("If", &condition)], bytes)?;
            Ok(())
        })();
        if operation.is_err() && lock.status == 201 {
            let _ = self.request("DELETE", path, &[("If", &condition)], None);
        }
        let unlock = self.checked("UNLOCK", path, &[("Lock-Token", &token)], None);
        operation?;
        unlock?;
        Ok(())
    }
    pub fn delete(&self, path: &str, etag: &str) -> Result<(), String> {
        if self.lock_writes {
            self.locked_write("DELETE", path, None, Some(etag))
        } else {
            self.checked("DELETE", path, &[("If-Match", etag)], None)
                .map(|_| ())
        }
    }
    pub fn probe(&mut self) -> Result<(), String> {
        let path = format!(".hematite-probe-{}.txt", glib::uuid_string_random());
        let result: Result<(), String> = (|| {
            self.checked("PUT", &path, &[], Some(b"first"))?;
            let stale = self
                .request(
                    "PUT",
                    &path,
                    &[("If-Match", "\"hematite-stale-version\"")],
                    Some(b"rejected"),
                )?
                .status;
            let duplicate = self
                .request("PUT", &path, &[("If-None-Match", "*")], Some(b"rejected"))?
                .status;
            let deletion = self
                .request(
                    "DELETE",
                    &path,
                    &[("If-Match", "\"hematite-stale-version\"")],
                    None,
                )?
                .status;
            if stale == 412 && duplicate == 412 && deletion == 412 {
                self.lock_writes = false;
                return Ok(());
            }
            self.checked("PUT", &path, &[], Some(b"first"))?;
            let lock = self.checked("LOCK", &path, &[("Content-Type", "application/xml"), ("Timeout", "Second-300")], Some(b"<d:lockinfo xmlns:d=\"DAV:\"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockinfo>"))?;
            let token = lock.lock_token.ok_or("Server cannot safely lock files")?;
            let verify: Result<(), String> = (|| {
                if self.request("PUT", &path, &[], Some(b"rejected"))?.status != 423
                    || self.request("DELETE", &path, &[], None)?.status != 423
                {
                    return Err(
                        "Server does not enforce write locks; safe sync is unavailable".into(),
                    );
                }
                self.checked(
                    "PUT",
                    &path,
                    &[("If", &format!("({token})"))],
                    Some(b"second"),
                )?;
                if self.get(&path)?.0 != b"second" {
                    return Err("Server write verification failed".into());
                }
                Ok(())
            })();
            let unlock = self.checked("UNLOCK", &path, &[("Lock-Token", &token)], None);
            verify?;
            unlock?;
            self.lock_writes = true;
            Ok(())
        })();
        let cleanup = self.request("DELETE", &path, &[], None);
        result?;
        if cleanup.is_err() {
            return Err("Could not clean up the server capability test".into());
        }
        Ok(())
    }
    pub fn list(&self) -> Result<BTreeMap<String, Remote>, String> {
        self.list_with_depth(true)
            .or_else(|_| self.list_with_depth(false))
    }

    fn list_with_depth(&self, recursive: bool) -> Result<BTreeMap<String, Remote>, String> {
        let mut pending = vec![String::new()];
        let mut seen = std::collections::HashSet::new();
        let mut files = BTreeMap::new();
        let base_uri =
            glib::Uri::parse(&self.url, glib::UriFlags::NONE).map_err(|e| e.to_string())?;
        let requested_root = format!("{}/", base_uri.path().trim_end_matches('/'));
        let mut wire_base = None::<String>;
        while let Some(folder) = pending.pop() {
            if !seen.insert(folder.clone()) {
                continue;
            }
            let response = self.checked("PROPFIND", &folder, &[("Depth", if recursive { "infinity" } else { "1" }), ("Content-Type", "application/xml")], Some(b"<d:propfind xmlns:d=\"DAV:\"><d:prop><d:resourcetype/><d:getetag/><d:getlastmodified/></d:prop></d:propfind>"))?;
            let items = parse_listing(&response.body)?;
            if folder.is_empty() {
                // A reverse proxy may strip /vault/ without rewriting DAV hrefs.
                // Learn the root collection's wire path from its own response.
                let root = items
                    .iter()
                    .filter(|item| item.collection)
                    .min_by_key(|item| item.href.matches('/').count())
                    .ok_or("Server listing did not include the vault collection")?;
                let path = if root.href.contains("://") {
                    glib::Uri::parse(&root.href, glib::UriFlags::NONE)
                        .map_err(|e| e.to_string())?
                        .path()
                        .to_string()
                } else {
                    root.href.clone()
                };
                let decoded = glib::uri_unescape_string(&path, None::<&str>)
                    .ok_or("Invalid vault collection URL")?;
                let root = format!("{}/", decoded.trim_end_matches('/'));
                if !requested_root.ends_with(&root) {
                    return Err("Incomplete WebDAV listing: root collection is missing".into());
                }
                wire_base = Some(root);
            }
            let mut found_self = false;
            for item in items {
                let href = if item.href.contains("://") {
                    let uri = glib::Uri::parse(&item.href, glib::UriFlags::NONE)
                        .map_err(|e| e.to_string())?;
                    if uri.host() != base_uri.host()
                        || uri.port() != base_uri.port()
                        || uri.scheme() != base_uri.scheme()
                    {
                        return Err("Server returned an external file URL".into());
                    }
                    uri.path().to_string()
                } else {
                    item.href
                };
                let decoded = glib::uri_unescape_string(&href, None::<&str>)
                    .ok_or("Invalid WebDAV file path")?;
                if decoded.trim_end_matches('/')
                    == wire_base.as_deref().unwrap().trim_end_matches('/')
                {
                    found_self |= folder.is_empty();
                    continue;
                }
                // Some proxies strip a URL prefix without rewriting response hrefs.
                let path = decoded
                    .strip_prefix(wire_base.as_deref().unwrap())
                    .ok_or("Server returned a file outside the vault")?
                    .trim_end_matches('/')
                    .to_string();
                if path == folder.trim_end_matches('/') {
                    found_self = true;
                    continue;
                }
                if !safe_path(&path) {
                    return Err("Server returned an unsafe file path".into());
                }
                if path.split('/').any(|part| part.starts_with('.')) {
                    continue;
                }
                if item.collection {
                    if !recursive {
                        pending.push(format!("{path}/"));
                    }
                } else {
                    if !item.etag.starts_with('"')
                        || !item.etag.ends_with('"')
                        || item.etag.contains(['\n', '\r'])
                    {
                        return Err("The server must provide strong ETags for safe syncing".into());
                    }
                    files.insert(
                        path,
                        Remote {
                            modified: parse_modified(&item.modified),
                            etag: item.etag,
                        },
                    );
                }
            }
            if !found_self {
                return Err("Incomplete WebDAV listing: collection response is missing".into());
            }
            if files.len() + seen.len() + pending.len() > 100_000 {
                return Err("Vault exceeds the sync file limit".into());
            }
        }
        Ok(files)
    }
}
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
fn parse_listing(xml: &[u8]) -> Result<Vec<Item>, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut entries = Vec::new();
    let mut item = None::<Item>;
    let mut field = Vec::new();
    let mut multistatus = false;
    let mut depth = 0usize;
    loop {
        match reader
            .read_event()
            .map_err(|e| format!("Invalid WebDAV listing: {e}"))?
        {
            Event::Start(tag) => {
                depth += 1;
                field = tag.local_name().as_ref().to_vec();
                if field == b"multistatus" {
                    multistatus = true;
                }
                if field == b"response" {
                    item = Some(Item::default());
                }
                if field == b"collection" {
                    if let Some(item) = &mut item {
                        item.collection = true;
                    }
                }
            }
            Event::Empty(tag) => {
                if tag.local_name().as_ref() == b"collection" {
                    if let Some(item) = &mut item {
                        item.collection = true;
                    }
                }
            }
            Event::Text(text) => {
                let decoded = text.decode().map_err(|e| e.to_string())?;
                let text = quick_xml::escape::unescape(&decoded).map_err(|e| e.to_string())?;
                if let Some(item) = &mut item {
                    match field.as_slice() {
                        b"href" => item.href.push_str(&text),
                        b"getetag" => item.etag.push_str(&text),
                        b"getlastmodified" => item.modified.push_str(&text),
                        b"status" => item.status.push(text.to_string()),
                        _ => {}
                    }
                }
            }
            Event::GeneralRef(reference) => {
                let character = if let Some(character) =
                    reference.resolve_char_ref().map_err(|e| e.to_string())?
                {
                    character
                } else {
                    match reference.as_ref() {
                        b"amp" => '&',
                        b"lt" => '<',
                        b"gt" => '>',
                        b"quot" => '"',
                        b"apos" => '\'',
                        _ => return Err("Unknown XML entity in WebDAV listing".into()),
                    }
                };
                if let Some(item) = &mut item {
                    match field.as_slice() {
                        b"href" => item.href.push(character),
                        b"getetag" => item.etag.push(character),
                        _ => {}
                    }
                }
            }
            Event::End(tag) => {
                depth = depth.checked_sub(1).ok_or("Invalid XML nesting")?;
                if tag.local_name().as_ref() == b"response" {
                    if let Some(item) = item.take() {
                        if !item.status.iter().any(|status| status.contains(" 200 ")) {
                            return Err(
                                "Incomplete WebDAV listing; no local files were removed".into()
                            );
                        }
                        entries.push(item);
                    }
                }
                field.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !multistatus || depth != 0 || item.is_some() {
        return Err("This address is not a WebDAV collection".into());
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_remote_modification_dates() {
        let expected = std::time::UNIX_EPOCH + std::time::Duration::from_secs(784111777);
        assert_eq!(
            parse_modified("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(expected)
        );
        assert_eq!(parse_modified("invalid"), None);
        assert_eq!(parse_modified("Sun, 99 Nov 1994 08:49:37 GMT"), None);
    }
    #[test]
    fn recursive_listing_and_depth_one_fallback() {
        use std::io::Read;
        use std::net::TcpListener;
        let root = "<d:response><d:href>/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>";
        let folder = root.replace("<d:href>/</d:href>", "<d:href>/folder/</d:href>");
        let file = "<d:response><d:href>/folder/note.md</d:href><d:propstat><d:prop><d:resourcetype/><d:getetag>&quot;v1&quot;</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>";
        for fallback in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let listings = if fallback {
                vec![
                    (403, String::new()),
                    (207, format!("{root}{folder}")),
                    (207, format!("{folder}{file}")),
                ]
            } else {
                vec![(207, format!("{root}{folder}{file}"))]
            };
            let server = std::thread::spawn(move || {
                for (index, (status, listing)) in listings.into_iter().enumerate() {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                        .unwrap();
                    let mut request = Vec::new();
                    loop {
                        let mut chunk = [0; 4096];
                        let read = stream.read(&mut chunk).unwrap();
                        assert!(read > 0);
                        request.extend_from_slice(&chunk[..read]);
                        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                            let length: usize = headers
                                .lines()
                                .find_map(|line| {
                                    line.strip_prefix("content-length:")
                                        .map(|value| value.trim().parse().unwrap())
                                })
                                .unwrap_or(0);
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    let headers = String::from_utf8_lossy(&request).to_lowercase();
                    assert!(headers.contains(if index == 0 {
                        "depth: infinity"
                    } else {
                        "depth: 1"
                    }));
                    let body = format!("<d:multistatus xmlns:d='DAV:'>{listing}</d:multistatus>");
                    write!(stream, "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let dav = Dav {
                url: format!("http://{address}/"),
                username: "test".into(),
                password: "test".into(),
                lock_writes: false,
            };
            let files = dav.list().unwrap();
            assert_eq!(files.len(), 1);
            assert_eq!(files["folder/note.md"].etag, "\"v1\"");
            server.join().unwrap();
        }
    }

    #[test]
    #[ignore = "Requires an explicitly supplied WebDAV server; read-only comparison"]
    fn live_recursive_listing_matches_folder_walk() {
        let dav = Dav {
            url: std::env::var("HEMATITE_TEST_WEBDAV_URL").unwrap(),
            username: std::env::var("HEMATITE_TEST_WEBDAV_USER").unwrap(),
            password: std::env::var("HEMATITE_TEST_WEBDAV_PASSWORD").unwrap(),
            lock_writes: false,
        };
        let start = std::time::Instant::now();
        let recursive = dav.list_with_depth(true).unwrap();
        let recursive_time = start.elapsed();
        let start = std::time::Instant::now();
        let walk = dav.list_with_depth(false).unwrap();
        let walk_time = start.elapsed();
        assert_eq!(
            recursive.keys().collect::<Vec<_>>(),
            walk.keys().collect::<Vec<_>>()
        );
        for (path, remote) in &recursive {
            assert_eq!(remote.etag, walk[path].etag);
        }
        println!(
            "Verified {} files: recursive {:.3}s, folder walk {:.3}s",
            recursive.len(),
            recursive_time.as_secs_f64(),
            walk_time.as_secs_f64()
        );
    }
    #[test]
    fn paths_and_names() {
        assert_eq!(
            normalize_url("example.org:8080/vault").unwrap(),
            "https://example.org:8080/vault/"
        );
        assert!(normalize_url("https://u:p@example.org/").is_err());
        assert_eq!(encode("café/one & two.md"), "caf%C3%A9/one%20%26%20two.md");
        for bad in ["../secret", "/etc/passwd", "folder/../note", "a\\b"] {
            assert!(!safe_path(bad));
        }
    }
    #[test]
    fn refuses_partial_listings() {
        let xml = b"<d:multistatus xmlns:d='DAV:'><d:response><d:href>/one</d:href><d:status>HTTP/1.1 403 Forbidden</d:status></d:response></d:multistatus>";
        assert!(parse_listing(xml).is_err());
    }
}
