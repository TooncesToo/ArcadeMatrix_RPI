//! GIF library management: network upload / browse / rename / delete of the playlist
//! folders under the gifs root. Mirrors the ESP32 firmware's `/api/gifs/*` endpoints.

use super::server::{check_auth, find_gif_candidate_roots, AppState};
use actix_multipart::Multipart;
use actix_web::{delete, get, post, web, HttpRequest, HttpResponse, Responder};
use futures_util::StreamExt;
use serde_json::json;
use std::path::Path;

// =====================================================================================
// GIF LIBRARY MANAGEMENT (parity with the ESP32 firmware: /api/gifs/*)
//   GET    /api/gifs/library                  -> live scan of the gifs root (folders + counts)
//   POST   /api/gifs/upload?folder=<Name>     -> multipart upload of one or many files
//   POST   /api/gifs/reindex                  -> the RPi scans live; shows an 'Indexing' notice on the panel, returns the library
//   GET    /api/gifs/reindex/status           -> same shape as the ESP32 (running/last_result/...)
//   DELETE /api/gifs/reindex                  -> mirrors the ESP32 cancel route (409: nothing to cancel here)
//   DELETE /api/gifs/file?folder=<N>&name=<F> -> delete one file
// =====================================================================================

pub fn gifs_root() -> std::path::PathBuf {
    find_gif_candidate_roots("yoko")
        .into_iter()
        .find(|p| p.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("gifs"))
}

/// Keep only a safe basename: letters, digits, `_ - space` (and `.` when an extension is allowed).
pub fn gifs_sanitize_name(input: &str, allow_ext: bool) -> String {
    let base = input.rsplit(|c| c == '/' || c == '\\').next().unwrap_or("");
    let filtered: String = base
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric()
                || *c == '_'
                || *c == '-'
                || *c == ' '
                || (allow_ext && *c == '.')
        })
        .collect();
    filtered
        .trim()
        .trim_start_matches('.')
        .chars()
        .take(64)
        .collect()
}

pub fn gifs_has_media_ext(name: &str) -> bool {
    let l = name.to_lowercase();
    l.ends_with(".gif") || l.ends_with(".png") || l.ends_with(".raw")
}

pub fn gifs_list_folder(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_file()
                && !name.starts_with('.')
                && name != "index.txt"
                && gifs_has_media_ext(&name)
            {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

pub fn gifs_scan_library() -> serde_json::Value {
    gifs_scan_library_at(&gifs_root())
}

/// Scan an explicit root (used by tests).
pub fn gifs_scan_library_at(root: &Path) -> serde_json::Value {
    let mut folders = serde_json::Map::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let count = gifs_list_folder(&root.join(&name)).len();
            folders.insert(
                name.clone(),
                json!({"path": format!("/gifs/{}", name), "count": count}),
            );
        }
    }
    json!({"root": "/gifs", "folders": folders})
}

#[get("/api/gifs/library")]
async fn get_gifs_library(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    HttpResponse::Ok().json(gifs_scan_library())
}

#[post("/api/gifs/reindex")]
async fn post_gifs_reindex(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    // The RPi GIF engine scans folders live, so there is nothing to rebuild; the scan itself takes well under a
    // second here. To behave like the ESP32 (where the same button runs for minutes) the panel still shows an
    // "Indexing library" notice for the duration, with a three-second minimum so it is actually visible.
    let started = std::time::Instant::now();
    REINDEX_RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
    data.config
        .set_message_payload(Some(crate::engines::message::MessagePayload::new(
            "Indexing GIF library...".to_string(),
            "#00ffff",
            1,
            "none", // static and centred: a scrolling message is still off-screen when the sub-second scan ends
            0,
        )));
    let mut lib = gifs_scan_library();
    let files: u64 = lib
        .get("folders")
        .and_then(|f| f.as_object())
        .map(|m| {
            m.values()
                .filter_map(|v| v.get("count").and_then(|c| c.as_u64()))
                .sum()
        })
        .unwrap_or(0);
    let folders = lib
        .get("folders")
        .and_then(|f| f.as_object())
        .map(|m| m.len())
        .unwrap_or(0);
    // keep the notice on screen for at least two seconds so the user sees the sign react
    let min_visible = std::time::Duration::from_millis(3000);
    if started.elapsed() < min_visible {
        actix_web::rt::time::sleep(min_visible - started.elapsed()).await;
    }
    data.config.set_message_payload(None);
    REINDEX_RUNNING.store(false, std::sync::atomic::Ordering::Relaxed);
    let result = format!("ok: {} folders, {} files", folders, files);
    *LAST_REINDEX.lock().unwrap() = (result.clone(), started.elapsed().as_millis() as u64, files);
    if let Some(obj) = lib.as_object_mut() {
        obj.insert("status".to_string(), json!("ok"));
        obj.insert("files".to_string(), json!(files));
        obj.insert("last_result".to_string(), json!(result));
    }
    HttpResponse::Ok().json(lib)
}

static REINDEX_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LAST_REINDEX: std::sync::Mutex<(String, u64, u64)> =
    std::sync::Mutex::new((String::new(), 0, 0));

/// Same shape as the ESP32 status route so the dashboard card can poll either firmware.
#[get("/api/gifs/reindex/status")]
async fn get_gifs_reindex_status(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let (result, elapsed, files) = LAST_REINDEX.lock().unwrap().clone();
    HttpResponse::Ok().json(json!({
        "running": REINDEX_RUNNING.load(std::sync::atomic::Ordering::Relaxed),
        "total": 0, "done": 0, "files": files, "expected": 0, "eta": "", "cancelling": false,
        "current": "", "last_result": result, "elapsed_ms": elapsed
    }))
}

/// Mirrors the ESP32 cancel route; the RPi scan is synchronous, so there is never anything to cancel.
#[delete("/api/gifs/reindex")]
async fn delete_gifs_reindex(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    HttpResponse::Conflict().json(json!({"status": "idle"}))
}

#[delete("/api/gifs/file")]
async fn delete_gifs_file(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw_folder = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    let raw_name = query.get("name").map(|s| s.as_str()).unwrap_or("");
    let folder = gifs_sanitize_name(raw_folder, false);
    let name = gifs_sanitize_name(raw_name, true);
    if raw_folder.contains('/')
        || raw_name.contains('/')
        || raw_folder.contains("..")
        || raw_name.contains("..")
        || folder.is_empty()
        || name.is_empty()
        || !gifs_has_media_ext(&name)
    {
        return HttpResponse::BadRequest().json(
            json!({"status": "error", "message": "folder and name (.gif/.png/.raw) are required"}),
        );
    }
    let path = gifs_root().join(&folder).join(&name);
    if !path.is_file() {
        return HttpResponse::NotFound()
            .json(json!({"status": "error", "message": "File not found"}));
    }
    match std::fs::remove_file(&path) {
        Ok(_) => HttpResponse::Ok()
            .json(json!({"status": "ok", "deleted": format!("/gifs/{}/{}", folder, name)})),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("Delete failed: {}", e)})),
    }
}

#[get("/api/gifs/files")]
async fn get_gifs_files(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    let folder = gifs_sanitize_name(raw, false);
    if raw.contains('/') || raw.contains("..") || folder.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "folder is required"}));
    }
    let dir = gifs_root().join(&folder);
    if !dir.is_dir() {
        return HttpResponse::NotFound()
            .json(json!({"status": "error", "message": "Folder not found"}));
    }
    let mut files = Vec::new();
    for name in gifs_list_folder(&dir) {
        let size = std::fs::metadata(dir.join(&name))
            .map(|m| m.len())
            .unwrap_or(0);
        files.push(json!({"name": name, "bytes": size}));
    }
    HttpResponse::Ok().json(json!({"folder": folder, "path": format!("/gifs/{}", folder), "count": files.len(), "files": files}))
}

#[delete("/api/gifs/folder")]
async fn delete_gifs_folder(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    let folder = gifs_sanitize_name(raw, false);
    if raw.contains('/') || raw.contains("..") || folder.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "folder is required"}));
    }
    let dir = gifs_root().join(&folder);
    if !dir.is_dir() {
        return HttpResponse::NotFound()
            .json(json!({"status": "error", "message": "Folder not found"}));
    }
    // count what we remove (whole playlist folder incl. subfolders such as Logo/256)
    let removed = walkdir_count(&dir);
    match std::fs::remove_dir_all(&dir) {
        Ok(_) => HttpResponse::Ok().json(
            json!({"status": "ok", "deleted": format!("/gifs/{}", folder), "files": removed}),
        ),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("Delete failed: {}", e)})),
    }
}

pub fn walkdir_count(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += walkdir_count(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}

pub fn gifs_bad(raw: &str) -> bool {
    raw.contains('/') || raw.contains('\\') || raw.contains("..")
}

#[post("/api/gifs/mkdir")]
async fn post_gifs_mkdir(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    let folder = gifs_sanitize_name(raw, false);
    if gifs_bad(raw) || folder.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "folder must be a plain name"}));
    }
    let dir = gifs_root().join(&folder);
    if dir.exists() {
        return HttpResponse::Conflict()
            .json(json!({"status": "error", "message": "Folder already exists"}));
    }
    match std::fs::create_dir_all(&dir) {
        Ok(_) => HttpResponse::Ok()
            .json(json!({"status": "ok", "folder": folder, "path": format!("/gifs/{}", folder)})),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("{}", e)})),
    }
}

/// Rename a file (folder + name + to) or a whole folder (folder + to, no name).
#[post("/api/gifs/rename")]
async fn post_gifs_rename(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw_folder = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    let raw_name = query.get("name").map(|s| s.as_str()).unwrap_or("");
    let raw_to = query.get("to").map(|s| s.as_str()).unwrap_or("");
    let folder = gifs_sanitize_name(raw_folder, false);
    if gifs_bad(raw_folder)
        || gifs_bad(raw_name)
        || gifs_bad(raw_to)
        || folder.is_empty()
        || raw_to.is_empty()
    {
        return HttpResponse::BadRequest().json(
            json!({"status": "error", "message": "folder and to are required (plain names)"}),
        );
    }
    let root = gifs_root();
    let (from, to, what) = if raw_name.is_empty() {
        let to_folder = gifs_sanitize_name(raw_to, false);
        if to_folder.is_empty() {
            return HttpResponse::BadRequest()
                .json(json!({"status": "error", "message": "bad target name"}));
        }
        (
            root.join(&folder),
            root.join(&to_folder),
            format!("/gifs/{}", to_folder),
        )
    } else {
        let name = gifs_sanitize_name(raw_name, true);
        let mut to_name = gifs_sanitize_name(raw_to, true);
        if !gifs_has_media_ext(&to_name) {
            // keep the original extension if the user typed a bare name
            if let Some(ext) = std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
            {
                to_name = format!("{}.{}", to_name, ext);
            }
        }
        if name.is_empty() || to_name.is_empty() || !gifs_has_media_ext(&to_name) {
            return HttpResponse::BadRequest()
                .json(json!({"status": "error", "message": "bad file name"}));
        }
        (
            root.join(&folder).join(&name),
            root.join(&folder).join(&to_name),
            format!("/gifs/{}/{}", folder, to_name),
        )
    };
    if !from.exists() {
        return HttpResponse::NotFound()
            .json(json!({"status": "error", "message": "Source not found"}));
    }
    if to.exists() {
        return HttpResponse::Conflict()
            .json(json!({"status": "error", "message": "Target already exists"}));
    }
    match std::fs::rename(&from, &to) {
        Ok(_) => HttpResponse::Ok().json(json!({"status": "ok", "renamed_to": what})),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("{}", e)})),
    }
}

/// Serve one media file (inline preview, or attachment with ?download=1).
#[get("/api/gifs/file")]
async fn get_gifs_file(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw_folder = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    let raw_name = query.get("name").map(|s| s.as_str()).unwrap_or("");
    let folder = gifs_sanitize_name(raw_folder, false);
    let name = gifs_sanitize_name(raw_name, true);
    if gifs_bad(raw_folder)
        || gifs_bad(raw_name)
        || folder.is_empty()
        || name.is_empty()
        || !gifs_has_media_ext(&name)
    {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "folder and name are required"}));
    }
    let path = gifs_root().join(&folder).join(&name);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&path)
                .first_or_octet_stream()
                .to_string();
            let mut resp = HttpResponse::Ok();
            resp.content_type(mime);
            resp.insert_header(("Cache-Control", "no-cache"));
            if query.get("download").map(|v| v == "1").unwrap_or(false) {
                resp.insert_header((
                    "Content-Disposition",
                    format!("attachment; filename=\"{}\"", name),
                ));
            }
            resp.body(bytes)
        }
        Err(_) => {
            HttpResponse::NotFound().json(json!({"status": "error", "message": "File not found"}))
        }
    }
}

#[post("/api/gifs/upload")]
async fn post_gifs_upload(
    req: HttpRequest,
    data: web::Data<AppState>,
    query: web::Query<std::collections::HashMap<String, String>>,
    mut payload: Multipart,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let raw_folder = query.get("folder").map(|s| s.as_str()).unwrap_or("");
    if raw_folder.contains('/') || raw_folder.contains('\\') || raw_folder.contains("..") {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "folder must be a plain playlist name (no path separators)"}));
    }
    let mut folder = gifs_sanitize_name(raw_folder, false);
    if folder.is_empty() {
        folder = "Uploads".to_string();
    }
    let dir = gifs_root().join(&folder);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return HttpResponse::InternalServerError().json(
            json!({"status": "error", "message": format!("Failed to create folder: {}", e)}),
        );
    }

    let mut saved: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();

    while let Some(item) = payload.next().await {
        let mut field = match item {
            Ok(f) => f,
            Err(e) => {
                return HttpResponse::BadRequest()
                    .json(json!({"status": "error", "message": format!("Upload error: {}", e)}));
            }
        };
        let raw_name = field
            .content_disposition()
            .and_then(|cd| cd.get_filename().map(|s| s.to_string()))
            .unwrap_or_default();
        if raw_name.is_empty() {
            // a plain form field, not a file: drain and ignore
            while let Some(_) = field.next().await {}
            continue;
        }
        let name = gifs_sanitize_name(&raw_name, true);
        if name.is_empty() || !gifs_has_media_ext(&name) {
            while let Some(_) = field.next().await {}
            skipped.push(json!({"name": raw_name, "reason": "unsupported type"}));
            continue;
        }
        let path = dir.join(&name);
        let mut file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                while let Some(_) = field.next().await {}
                skipped.push(json!({"name": name, "reason": format!("open failed: {}", e)}));
                continue;
            }
        };
        let mut bytes: usize = 0;
        let mut ok = true;
        while let Some(chunk) = field.next().await {
            match chunk {
                Ok(d) => {
                    use std::io::Write;
                    if file.write_all(&d).is_err() {
                        ok = false;
                        break;
                    }
                    bytes += d.len();
                }
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        drop(file);
        if ok && bytes > 0 {
            saved.push(json!({"name": name, "bytes": bytes}));
        } else {
            let _ = std::fs::remove_file(&path);
            skipped.push(json!({"name": name, "reason": "write failed"}));
        }
    }

    let status = if saved.is_empty() { "error" } else { "ok" };
    let body = json!({"status": status, "folder": folder, "count": saved.len(), "saved": saved, "skipped": skipped});
    if saved.is_empty() {
        HttpResponse::BadRequest().json(body)
    } else {
        HttpResponse::Ok().json(body)
    }
}
