use crate::core::config::{Config, EngineInstance, RotationEntry};
use crate::core::config_sanitizer::ConfigSanitizer;
use crate::core::registry::EngineRegistry;
use actix_multipart::Multipart;
use actix_web::{delete, get, post, web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use futures_util::StreamExt;
use rust_embed::RustEmbed;
use serde_json::json;
use std::net::UdpSocket;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use sysinfo::System;

pub struct AppState {
    pub config: Arc<Config>,
}

/// Best-effort local IPv4 discovery (used to tell a game console where to push
/// MQTT marquee events). Falls back to loopback when offline.
fn get_local_ip() -> String {
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

fn check_auth(req: &HttpRequest, config: &Config) -> Result<(), HttpResponse> {
    let s = config.settings.read();
    if !s.api_auth_enabled {
        return Ok(());
    }

    let supplied = req
        .headers()
        .get("X-API-Token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if supplied.is_empty() || supplied != s.api_token {
        return Err(HttpResponse::Unauthorized().json(json!({
            "status": "error",
            "message": "Missing or invalid X-API-Token header"
        })));
    }
    Ok(())
}

#[get("/api/system")]
async fn get_system(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let s = data.config.settings.read();
    HttpResponse::Ok().json(json!({
        "system": s.system,
        "matrix": s.matrix,
        "mqtt": s.mqtt,
        "wifi": s.wifi,
        "api_auth_enabled": s.api_auth_enabled,
        "api_token": s.api_token
    }))
}

#[post("/api/system")]
async fn post_system(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let mut s = data.config.settings.write();
    // Snapshot the fields that require a hardware/service restart to take effect.
    let prev_matrix = serde_json::to_value(&s.matrix).ok();
    let prev_mqtt = serde_json::to_value(&s.mqtt).ok();
    let prev_disable_wifi = s.wifi.disable_internal;
    // Simplified update (normally you'd merge the fields)
    if let Some(sys) = body.get("system") {
        if let Ok(sys_val) = serde_json::from_value(sys.clone()) {
            s.system = sys_val;
        }
    }
    if let Some(mat) = body.get("matrix") {
        if let Ok(mat_val) = serde_json::from_value(mat.clone()) {
            s.matrix = mat_val;
        }
    }
    if let Some(mq) = body.get("mqtt") {
        if let Ok(mq_val) = serde_json::from_value(mq.clone()) {
            s.mqtt = mq_val;
        }
    }
    if let Some(wf) = body.get("wifi") {
        if let Ok(wf_val) = serde_json::from_value(wf.clone()) {
            s.wifi = wf_val;
        }
    }
    // Top-level (non-nested) settings sent directly by the web UI.
    if let Some(v) = body.get("api_auth_enabled").and_then(|v| v.as_bool()) {
        s.api_auth_enabled = v;
    }
    if let Some(v) = body.get("api_token").and_then(|v| v.as_str()) {
        s.api_token = v.to_string();
    }
    if let Some(v) = body.get("lang").and_then(|v| v.as_str()) {
        s.system.lang = v.to_string();
    }
    if let Some(v) = body.get("timezone").and_then(|v| v.as_str()) {
        s.system.timezone = v.to_string();
    }
    if let Some(v) = body.get("format_24h").and_then(|v| v.as_bool()) {
        s.system.format_24h = v;
    }
    if let Some(v) = body.get("temp_unit").and_then(|v| v.as_str()) {
        s.system.temp_unit = v.to_string();
    }
    // Fighter overlay toggle/interval (media page). Handled as top-level keys so
    // the UI can patch them without replacing the whole `system` object.
    if let Some(v) = body.get("idle_fighter_enabled").and_then(|v| v.as_bool()) {
        s.system.idle_fighter_enabled = v;
    }
    if let Some(v) = body.get("idle_fighter_interval").and_then(|v| v.as_u64()) {
        s.system.idle_fighter_interval = (v.max(1)) as u32;
    }
    if let Some(v) = body.get("idle_fighter_speed").and_then(|v| v.as_u64()) {
        s.system.idle_fighter_speed = (v.clamp(25, 200)) as u32;
    }
    if let Some(v) = body.get("night_mode_enabled").and_then(|v| v.as_bool()) {
        s.system.night_mode_enabled = v;
    }
    if let Some(v) = body.get("turn_off_at").and_then(|v| v.as_str()) {
        s.system.turn_off_at = v.to_string();
    }
    if let Some(v) = body.get("wake_up_at").and_then(|v| v.as_str()) {
        s.system.wake_up_at = v.to_string();
    }
    if let Some(v) = body.get("night_brightness").and_then(|v| v.as_u64()) {
        s.system.night_brightness = (v.min(100)) as u32;
    }
    // Live daytime brightness (0-100), applied immediately without a restart.
    if let Some(v) = body
        .get("brightness_limit")
        .or_else(|| body.get("brightness"))
        .and_then(|v| v.as_u64())
    {
        let clamped = v.min(100) as u32;
        data.config
            .matrix_brightness
            .store(clamped, Ordering::Relaxed);
        // Persist it too, so the saved brightness survives a restart instead of
        // reverting to the default when the config is reloaded from disk.
        s.system.day_brightness = clamped;
    }
    // Real-time display orientation & transitions (applied immediately without a restart).
    if let Some(v) = body.get("rotation").and_then(|v| v.as_u64()) {
        if s.matrix.rotation != v as u32 {
            s.matrix.rotation = v as u32;
            data.config.reset_rotation.store(true, Ordering::Relaxed);
        }
    }
    if let Some(v) = body.get("transition_effect").and_then(|v| v.as_str()) {
        s.matrix.transition_effect = v.to_string();
    }
    if let Some(v) = body.get("transition_duration_ms").and_then(|v| v.as_u64()) {
        s.matrix.transition_duration_ms = v as u32;
    }
    // Hardware & MQTT-affecting settings only take effect after a restart of the
    // render/network loops, so flag it when physical matrix params, MQTT broker, or Wi-Fi state change.
    let prev_matrix_cfg: Option<crate::core::config::MatrixConfig> =
        prev_matrix.and_then(|v| serde_json::from_value(v).ok());
    let hw_matrix_changed = if let Some(ref p) = prev_matrix_cfg {
        p.width != s.matrix.width
            || p.height != s.matrix.height
            || p.chain_length != s.matrix.chain_length
            || p.driver_chip != s.matrix.driver_chip
            || p.mapping != s.matrix.mapping
            || p.rgb_sequence != s.matrix.rgb_sequence
            || p.row_address_mode != s.matrix.row_address_mode
            || p.multiplexing != s.matrix.multiplexing
            || p.slowdown != s.matrix.slowdown
            || p.pwm_bits != s.matrix.pwm_bits
            || p.pwm_lsb_nanoseconds != s.matrix.pwm_lsb_nanoseconds
            || p.limit_refresh_rate_hz != s.matrix.limit_refresh_rate_hz
            || p.disable_hardware_pulsing != s.matrix.disable_hardware_pulsing
    } else {
        false
    };
    if let Some(ref p) = prev_matrix_cfg {
        if p.rotation != s.matrix.rotation {
            data.config.reset_rotation.store(true, Ordering::Relaxed);
        }
    }
    let new_mqtt = serde_json::to_value(&s.mqtt).ok();
    let needs_reload =
        hw_matrix_changed || prev_mqtt != new_mqtt || prev_disable_wifi != s.wifi.disable_internal;
    drop(s);
    data.config.save();
    if needs_reload {
        data.config.reload_flag.store(true, Ordering::Relaxed);
    }
    HttpResponse::Ok().json(json!({"status": if needs_reload { "rebooting" } else { "ok" }}))
}

#[get("/api/instances")]
async fn get_instances(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let s = data.config.settings.read();
    HttpResponse::Ok().json(&s.instances)
}

#[post("/api/instances")]
async fn post_instances(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<EngineInstance>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let mut new_inst = body.into_inner();
    if new_inst.engine_id == "sysinfo" || new_inst.engine_id == "sys_info" {
        new_inst.engine_id = "system_info".to_string();
    } else if new_inst.engine_id == "gif" {
        new_inst.engine_id = "gifs".to_string();
    } else if new_inst.engine_id == "cast" {
        new_inst.engine_id = "google_cast".to_string();
    }
    // Reject instances referencing an engine that isn't registered in the
    // auto-discovery registry: they would silently never render.
    if EngineRegistry::get_descriptor(&new_inst.engine_id).is_none() {
        return HttpResponse::BadRequest().json(json!({
            "status": "error",
            "message": format!("Unknown engine_id '{}'", new_inst.engine_id)
        }));
    }
    let mut s = data.config.settings.write();
    if let Some(existing) = s
        .instances
        .iter_mut()
        .find(|i| i.instance_id == new_inst.instance_id)
    {
        *existing = new_inst;
    } else {
        s.instances.push(new_inst);
    }
    // Self-heal at write time: inject defaults, clamp/fallback out-of-range values.
    ConfigSanitizer::sanitize_instances(&mut s);
    drop(s);
    data.config.save();
    // Make the runtime pick up the new/edited instance immediately.
    data.config.reset_rotation.store(true, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({"status": "ok"}))
}

#[delete("/api/instances/{id}")]
async fn delete_instance(
    req: HttpRequest,
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let mut s = data.config.settings.write();
    s.instances.retain(|i| i.instance_id != *path);
    // Drop any rotation entries that referenced the removed instance so the
    // render loop never points at a dangling instance_id.
    s.rotation.retain(|r| r.instance_id != *path);
    drop(s);
    data.config.save();
    data.config.reset_rotation.store(true, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({"status": "ok"}))
}

#[get("/api/rotation")]
async fn get_rotation(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let s = data.config.settings.read();
    HttpResponse::Ok().json(&s.rotation)
}

#[post("/api/rotation")]
async fn post_rotation(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<Vec<RotationEntry>>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let mut rot = body.into_inner();
    for entry in &mut rot {
        entry.normalize();
    }
    let mut s = data.config.settings.write();
    s.rotation = rot;
    drop(s);
    data.config.save();
    data.config.reset_rotation.store(true, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({"status": "ok"}))
}

#[get("/api/engines")]
async fn get_engines(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    HttpResponse::Ok().json(crate::core::registry::EngineRegistry::get_all_descriptors())
}

/// Runs a privileged power command, trying several candidates so it works
/// whether or not `sudo` is needed and regardless of the systemd PATH. Verifies
/// command exit status and logs stderr before falling back to the next candidate.
fn run_power_command(candidates: &[&[&str]]) -> bool {
    for cand in candidates {
        let (bin, args) = cand.split_first().expect("non-empty command");
        match std::process::Command::new(bin).args(args).output() {
            Ok(output) => {
                if output.status.success() {
                    tracing::info!("Power command executed successfully: {} {:?}", bin, args);
                    return true;
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    tracing::warn!(
                        "Power command '{} {:?}' exited with status {}: {}",
                        bin,
                        args,
                        output.status,
                        err.trim()
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Power command '{}' failed to start: {}", bin, e);
            }
        }
    }
    tracing::error!("All power command candidates failed");
    false
}

#[post("/api/system/restart")]
async fn post_restart(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    // Gracefully stop the render loop; the process then exits and systemd
    // (Restart=always) brings the app back in a couple of seconds. This applies
    // hardware-level settings without a full OS reboot.
    tracing::info!("Application restart requested via API.");
    data.config.reload_flag.store(true, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({"status": "restarting"}))
}

#[get("/api/action/reboot")]
async fn reboot(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let ok = run_power_command(&[
        &["systemctl", "reboot"],
        &["sudo", "systemctl", "reboot"],
        &["/bin/systemctl", "reboot"],
        &["/usr/bin/systemctl", "reboot"],
        &["reboot"],
        &["sudo", "reboot"],
        &["/sbin/reboot"],
        &["sudo", "/sbin/reboot"],
        &["shutdown", "-r", "now"],
        &["sudo", "shutdown", "-r", "now"],
        &["/sbin/shutdown", "-r", "now"],
        &["sudo", "/sbin/shutdown", "-r", "now"],
    ]);
    if ok {
        HttpResponse::Ok().json(json!({"status": "rebooting"}))
    } else {
        HttpResponse::InternalServerError().json(json!({"status": "error"}))
    }
}

#[post("/api/system/reboot")]
async fn post_reboot(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let ok = run_power_command(&[
        &["systemctl", "reboot"],
        &["sudo", "systemctl", "reboot"],
        &["/bin/systemctl", "reboot"],
        &["/usr/bin/systemctl", "reboot"],
        &["reboot"],
        &["sudo", "reboot"],
        &["/sbin/reboot"],
        &["sudo", "/sbin/reboot"],
        &["shutdown", "-r", "now"],
        &["sudo", "shutdown", "-r", "now"],
        &["/sbin/shutdown", "-r", "now"],
        &["sudo", "/sbin/shutdown", "-r", "now"],
    ]);
    if ok {
        HttpResponse::Ok().json(json!({"status": "rebooting"}))
    } else {
        HttpResponse::InternalServerError().json(json!({"status": "error"}))
    }
}

#[post("/api/system/shutdown")]
async fn post_shutdown(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let ok = run_power_command(&[
        &["systemctl", "poweroff"],
        &["sudo", "systemctl", "poweroff"],
        &["/bin/systemctl", "poweroff"],
        &["/usr/bin/systemctl", "poweroff"],
        &["poweroff"],
        &["sudo", "poweroff"],
        &["/sbin/poweroff"],
        &["sudo", "/sbin/poweroff"],
        &["shutdown", "-h", "now"],
        &["sudo", "shutdown", "-h", "now"],
        &["/sbin/shutdown", "-h", "now"],
        &["sudo", "/sbin/shutdown", "-h", "now"],
    ]);
    if ok {
        HttpResponse::Ok().json(json!({"status": "shutting_down"}))
    } else {
        HttpResponse::InternalServerError().json(json!({"status": "error"}))
    }
}

#[post("/api/system/power")]
async fn post_power(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    // `state = true` means the panel should be ON.
    let state = body.get("state").and_then(|v| v.as_bool()).unwrap_or(true);
    data.config.matrix_power.store(state, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({"status": "ok", "power": state}))
}

#[get("/api/stats")]
async fn get_stats(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }

    // Gather CPU/memory/disk/temperature off the async reactor thread.
    let stats = web::block(|| {
        let mut sys = System::new_all();
        // Two samples spaced by the minimum interval for a meaningful CPU load.
        sys.refresh_cpu_all();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_all();
        sys.refresh_memory();

        let cpu_load = sys.global_cpu_usage();
        let ram_used_mb = (sys.used_memory() / (1024 * 1024)) as u64;
        let ram_total_mb = (sys.total_memory() / (1024 * 1024)) as u64;

        // RPi exposes the SoC temperature via the thermal sysfs entry.
        let temperature_c = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map(|milli| milli / 1000.0)
            .unwrap_or(0.0);

        let disks = sysinfo::Disks::new_with_refreshed_list();
        let disk_free_gb = disks
            .list()
            .iter()
            .find(|d| d.mount_point() == std::path::Path::new("/"))
            .or_else(|| disks.list().first())
            .map(|d| d.available_space() as f64 / (1024.0 * 1024.0 * 1024.0))
            .unwrap_or(0.0);

        (
            cpu_load,
            ram_used_mb,
            ram_total_mb,
            temperature_c,
            disk_free_gb,
        )
    })
    .await
    .unwrap_or((0.0, 0, 0, 0.0, 0.0));

    HttpResponse::Ok().json(json!({
        "version": crate::core::build_info::VERSION,
        "arch": crate::core::build_info::ARCH,
        "uptime": sysinfo::System::uptime(),
        "cpu_load": stats.0,
        "ram_used_mb": stats.1,
        "ram_total_mb": stats.2,
        "memory_used": stats.1,
        "memory_total": stats.2,
        "temperature_c": stats.3,
        "disk_free_gb": (stats.4 * 10.0).round() / 10.0
    }))
}

#[post("/api/wifi")]
async fn post_wifi(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let ssid = body.get("ssid").and_then(|v| v.as_str()).unwrap_or("");
    let password = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
    if ssid.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "SSID is required"}));
    }
    {
        let mut s = data.config.settings.write();
        s.wifi.ssid = ssid.to_string();
        s.wifi.password = password.to_string();
        // Force the startup routine to (re)apply the NetworkManager profile.
        s.wifi.configured = false;
    }
    data.config.save();
    // Restart so the boot Wi-Fi provisioning block re-runs with the new profile.
    data.config.reload_flag.store(true, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({"status": "ok"}))
}

async fn handle_file_upload(
    req: HttpRequest,
    data: web::Data<AppState>,
    mut payload: Multipart,
) -> HttpResponse {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let mut bytes = Vec::new();
    let mut file_ext = String::new();

    while let Some(item) = payload.next().await {
        let mut field = match item {
            Ok(f) => f,
            Err(e) => {
                return HttpResponse::BadRequest()
                    .json(json!({"status": "error", "message": format!("Upload error: {}", e)}));
            }
        };

        if file_ext.is_empty() {
            if let Some(cd) = field.content_disposition() {
                if let Some(fname) = cd.get_filename() {
                    let p = std::path::Path::new(fname);
                    if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                        file_ext = format!(".{}", ext.to_lowercase());
                    }
                }
            }
        }

        while let Some(chunk) = field.next().await {
            match chunk {
                Ok(d) => bytes.extend_from_slice(&d),
                Err(e) => {
                    return HttpResponse::BadRequest().json(
                        json!({"status": "error", "message": format!("Chunk error: {}", e)}),
                    );
                }
            }
        }
    }

    if bytes.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "No file received"}));
    }

    if file_ext.is_empty() {
        if bytes.starts_with(b"GIF8") {
            file_ext = ".gif".to_string();
        } else if bytes.starts_with(b"\x89PNG") {
            file_ext = ".png".to_string();
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            file_ext = ".jpg".to_string();
        } else {
            file_ext = ".gif".to_string();
        }
    }

    let is_marquee =
        req.path().contains("marquee") || req.query_string().contains("target=marquee");
    let target_dir = if is_marquee {
        std::path::Path::new("data/marquees")
    } else {
        std::path::Path::new("data/uploads")
    };
    if let Err(e) = std::fs::create_dir_all(target_dir) {
        return HttpResponse::InternalServerError().json(
            json!({"status": "error", "message": format!("Failed to create directory: {}", e)}),
        );
    }

    if is_marquee {
        // Enforce single-file overwrite: clear existing marquee files
        if let Ok(entries) = std::fs::read_dir(target_dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let dest_path = if is_marquee {
        target_dir.join(format!("marquee{}", file_ext))
    } else {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        target_dir.join(format!("asset_{}{}", ts, file_ext))
    };

    if let Err(e) = std::fs::write(&dest_path, &bytes) {
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("Failed to save file: {}", e)}));
    }

    // Clear stale static image buffer if marquee
    if is_marquee {
        *data.config.image_obj.lock() = None;
        // Do NOT set ForcedEngineMode::Marquee: Marquee runs normally in the rotation loop
    }

    HttpResponse::Ok().json(json!({
        "status": "ok",
        "success": true,
        "path": dest_path.to_string_lossy().to_string(),
        "message": "File uploaded successfully"
    }))
}

#[post("/api/marquee")]
async fn post_marquee(
    req: HttpRequest,
    data: web::Data<AppState>,
    payload: Multipart,
) -> impl Responder {
    handle_file_upload(req, data, payload).await
}

#[post("/api/upload")]
async fn post_upload(
    req: HttpRequest,
    data: web::Data<AppState>,
    payload: Multipart,
) -> impl Responder {
    handle_file_upload(req, data, payload).await
}

#[post("/api/mqtt/install")]
async fn post_mqtt_install(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let ip = body
        .get("ip")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if ip.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "Console IP is required"}));
    }
    let user = body
        .get("user")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pass = body
        .get("pass")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let target_os = body
        .get("os")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let matrix_ip = get_local_ip();

    let res = web::block(move || {
        crate::core::ssh_installer::install_sync_script(&ip, &matrix_ip, user, pass, target_os)
    })
    .await;

    match res {
        Ok(Ok(log)) => HttpResponse::Ok().json(json!({"status": "ok", "log": log})),
        Ok(Err(msg)) => HttpResponse::BadRequest().json(json!({"status": "error", "message": msg})),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": e.to_string()})),
    }
}

#[post("/api/mqtt/logs")]
async fn post_mqtt_logs(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    let ip = body
        .get("ip")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if ip.is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "Console IP is required"}));
    }
    let user = body
        .get("user")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pass = body
        .get("pass")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let target_os = body
        .get("os")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let res =
        web::block(move || crate::core::ssh_installer::fetch_sync_logs(&ip, user, pass, target_os))
            .await;

    match res {
        Ok(Ok(logs)) => HttpResponse::Ok().json(json!({"status": "ok", "logs": logs})),
        Ok(Err(msg)) => HttpResponse::BadRequest().json(json!({"status": "error", "message": msg})),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": e.to_string()})),
    }
}

#[get("/api/fonts")]
async fn get_fonts(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }

    let mut fonts = Vec::new();
    if let Ok(entries) = std::fs::read_dir("fonts") {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_file() {
                    if let Some(ext) = entry.path().extension() {
                        if ext == "ttf" || ext == "bdf" {
                            if let Some(name) = entry.file_name().to_str() {
                                fonts.push(json!({"value": name, "label": name}));
                            }
                        }
                    }
                }
            }
        }
    }
    HttpResponse::Ok().json(fonts)
}

fn find_gif_candidate_roots(orientation: &str) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/pi".to_string());

    let subs: &[&str] = if orientation == "tate" {
        &["gifs_tate", "gifs/tate", "tate"]
    } else {
        &["gifs", "gifs/yoko", "yoko"]
    };

    let base_prefixes = [
        "",
        "/",
        "data/",
        "release/sdCard/",
        "../",
        "../ArcadeMatrix/release/sdCard/",
        "/opt/arcadematrix/",
        &format!("{}/", home),
        &format!("{}/ArcadeMatrix_RPi/", home),
        &format!("{}/ArcadeMatrix/", home),
        &format!("{}/ArcadeMatrix/release/sdCard/", home),
        "/media/",
        "/mnt/",
    ];

    for base in &base_prefixes {
        for sub in subs {
            let candidate = if base.is_empty() {
                std::path::PathBuf::from(sub)
            } else if base.starts_with('/') {
                std::path::Path::new(base).join(sub)
            } else {
                std::path::PathBuf::from(format!("{}{}", base, sub))
            };

            if candidate.is_dir() && !roots.contains(&candidate) {
                roots.push(candidate);
            }
        }
    }

    roots
}

fn count_gifs_in_dir(path: &std::path::Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    count += count_gifs_in_dir(&entry.path());
                } else {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.to_lowercase().ends_with(".gif") && !name.starts_with("._") {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

#[get("/api/playlists")]
async fn get_playlists(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }

    let mut playlists = Vec::new();
    let mut seen_yoko = std::collections::HashSet::new();
    let mut seen_tate = std::collections::HashSet::new();

    // 1. Scan horizontal / yoko folders across candidate roots
    for root in find_gif_candidate_roots("yoko") {
        let pl_json = root.join("playlists.json");
        if pl_json.is_file() {
            if let Ok(content) = std::fs::read_to_string(&pl_json) {
                if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = doc.as_object() {
                        for (k, v) in obj {
                            if !k.starts_with('.')
                                && !k.starts_with('_')
                                && seen_yoko.insert(k.clone())
                            {
                                let path = v.get("path").and_then(|p| p.as_str()).unwrap_or(k);
                                playlists.push(json!({
                                    "value": if path.starts_with('/') { path.to_string() } else { format!("/gifs/{}", path) },
                                    "label": k,
                                    "orientation": "yoko"
                                }));
                            }
                        }
                    }
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.')
                                && !name.starts_with('_')
                                && seen_yoko.insert(name.to_string())
                            {
                                playlists.push(json!({
                                    "value": format!("/gifs/{}", name),
                                    "label": name,
                                    "orientation": "yoko"
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Scan vertical / tate folders across candidate roots
    for root in find_gif_candidate_roots("tate") {
        let pl_json = root.join("playlists.json");
        if pl_json.is_file() {
            if let Ok(content) = std::fs::read_to_string(&pl_json) {
                if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = doc.as_object() {
                        for (k, v) in obj {
                            if !k.starts_with('.')
                                && !k.starts_with('_')
                                && seen_tate.insert(k.clone())
                            {
                                let path = v.get("path").and_then(|p| p.as_str()).unwrap_or(k);
                                playlists.push(json!({
                                    "value": if path.starts_with('/') { path.to_string() } else { format!("/gifs_tate/{}", path) },
                                    "label": k,
                                    "orientation": "tate"
                                }));
                            }
                        }
                    }
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.')
                                && !name.starts_with('_')
                                && seen_tate.insert(name.to_string())
                            {
                                playlists.push(json!({
                                    "value": format!("/gifs_tate/{}", name),
                                    "label": name,
                                    "orientation": "tate"
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    HttpResponse::Ok().json(playlists)
}

#[get("/api/gifs/playlists")]
async fn get_gifs_playlists(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }

    let mut yoko_map = serde_json::Map::new();
    for root in find_gif_candidate_roots("yoko") {
        let pl_json = root.join("playlists.json");
        if pl_json.is_file() {
            if let Ok(content) = std::fs::read_to_string(&pl_json) {
                if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = doc.as_object() {
                        for (k, v) in obj {
                            if !k.starts_with('.')
                                && !k.starts_with('_')
                                && !yoko_map.contains_key(k)
                            {
                                yoko_map.insert(k.clone(), v.clone());
                            }
                        }
                    }
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.')
                                && !name.starts_with('_')
                                && !yoko_map.contains_key(name)
                            {
                                let count = count_gifs_in_dir(&entry.path());
                                yoko_map.insert(
                                    name.to_string(),
                                    json!({"path": format!("/gifs/{}", name), "count": count}),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    let mut tate_map = serde_json::Map::new();
    for root in find_gif_candidate_roots("tate") {
        let pl_json = root.join("playlists.json");
        if pl_json.is_file() {
            if let Ok(content) = std::fs::read_to_string(&pl_json) {
                if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = doc.as_object() {
                        for (k, v) in obj {
                            if !k.starts_with('.')
                                && !k.starts_with('_')
                                && !tate_map.contains_key(k)
                            {
                                tate_map.insert(k.clone(), v.clone());
                            }
                        }
                    }
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.')
                                && !name.starts_with('_')
                                && !tate_map.contains_key(name)
                            {
                                let count = count_gifs_in_dir(&entry.path());
                                tate_map.insert(
                                    name.to_string(),
                                    json!({"path": format!("/gifs_tate/{}", name), "count": count}),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    HttpResponse::Ok().json(json!({
        "yoko": yoko_map,
        "tate": tate_map
    }))
}

#[get("/api/themes")]
async fn get_themes(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    if let Err(e) = check_auth(&req, &data.config) {
        return e;
    }
    // Built automatically from the single theme source of truth: adding a new
    // ThemeInfo entry in core::theme makes it appear in the UI dropdown with no
    // other change required here.
    let themes: Vec<_> = crate::core::theme::all_themes()
        .iter()
        .map(|t| json!({"value": t.id.to_string(), "label": t.name}))
        .collect();
    HttpResponse::Ok().json(themes)
}

#[get("/api/timezones")]
async fn get_timezones() -> impl actix_web::Responder {
    let zones = [
        ("Europe/Paris", "Europe/Paris (UTC+1/+2)"),
        ("Europe/London", "Europe/London (UTC+0/+1)"),
        ("Europe/Dublin", "Europe/Dublin (UTC+0/+1)"),
        ("Europe/Lisbon", "Europe/Lisbon (UTC+0/+1)"),
        ("Europe/Berlin", "Europe/Berlin (UTC+1/+2)"),
        ("Europe/Madrid", "Europe/Madrid (UTC+1/+2)"),
        ("Europe/Rome", "Europe/Rome (UTC+1/+2)"),
        ("Europe/Brussels", "Europe/Brussels (UTC+1/+2)"),
        ("Europe/Amsterdam", "Europe/Amsterdam (UTC+1/+2)"),
        ("Europe/Zurich", "Europe/Zurich (UTC+1/+2)"),
        ("Europe/Vienna", "Europe/Vienna (UTC+1/+2)"),
        ("Europe/Warsaw", "Europe/Warsaw (UTC+1/+2)"),
        ("Europe/Prague", "Europe/Prague (UTC+1/+2)"),
        ("Europe/Stockholm", "Europe/Stockholm (UTC+1/+2)"),
        ("Europe/Oslo", "Europe/Oslo (UTC+1/+2)"),
        ("Europe/Copenhagen", "Europe/Copenhagen (UTC+1/+2)"),
        ("Europe/Athens", "Europe/Athens (UTC+2/+3)"),
        ("Europe/Helsinki", "Europe/Helsinki (UTC+2/+3)"),
        ("Europe/Bucharest", "Europe/Bucharest (UTC+2/+3)"),
        ("Europe/Kyiv", "Europe/Kyiv (UTC+2/+3)"),
        ("Europe/Moscow", "Europe/Moscow (UTC+3)"),
        ("Europe/Istanbul", "Europe/Istanbul (UTC+3)"),
        ("Atlantic/Reykjavik", "Atlantic/Reykjavik (UTC+0)"),
        ("Atlantic/Azores", "Atlantic/Azores (UTC-1/+0)"),
        ("America/New_York", "America/New_York (EST/EDT, UTC-5/-4)"),
        ("America/Detroit", "America/Detroit (EST/EDT, UTC-5/-4)"),
        (
            "America/Indiana/Indianapolis",
            "America/Indiana/Indianapolis (EST/EDT, UTC-5/-4)",
        ),
        ("America/Montreal", "America/Montreal (EST/EDT, UTC-5/-4)"),
        ("America/Toronto", "America/Toronto (EST/EDT, UTC-5/-4)"),
        ("America/Chicago", "America/Chicago (CST/CDT, UTC-6/-5)"),
        ("America/Mexico_City", "America/Mexico_City (CST, UTC-6)"),
        ("America/Denver", "America/Denver (MST/MDT, UTC-7/-6)"),
        ("America/Boise", "America/Boise (MST/MDT, UTC-7/-6)"),
        ("America/Phoenix", "America/Phoenix (MST, UTC-7, no DST)"),
        (
            "America/Los_Angeles",
            "America/Los_Angeles (PST/PDT, UTC-8/-7)",
        ),
        ("America/Vancouver", "America/Vancouver (PST/PDT, UTC-8/-7)"),
        (
            "America/Anchorage",
            "America/Anchorage (AKST/AKDT, UTC-9/-8)",
        ),
        ("America/Halifax", "America/Halifax (AST/ADT, UTC-4/-3)"),
        (
            "America/St_Johns",
            "America/St_Johns (NST/NDT, UTC-3:30/-2:30)",
        ),
        ("Pacific/Honolulu", "Pacific/Honolulu (HST, UTC-10)"),
        ("America/Sao_Paulo", "America/Sao_Paulo (BRT, UTC-3)"),
        ("America/Buenos_Aires", "America/Buenos_Aires (ART, UTC-3)"),
        ("America/Santiago", "America/Santiago (CLT/CLST, UTC-4/-3)"),
        ("America/Bogota", "America/Bogota (COT, UTC-5)"),
        ("America/Lima", "America/Lima (PET, UTC-5)"),
        ("Africa/Casablanca", "Africa/Casablanca (WEST, UTC+1)"),
        ("Africa/Cairo", "Africa/Cairo (EET/EEST, UTC+2/+3)"),
        ("Africa/Johannesburg", "Africa/Johannesburg (SAST, UTC+2)"),
        ("Africa/Nairobi", "Africa/Nairobi (EAT, UTC+3)"),
        ("Africa/Lagos", "Africa/Lagos (WAT, UTC+1)"),
        ("Asia/Jerusalem", "Asia/Jerusalem (IST/IDT, UTC+2/+3)"),
        ("Asia/Riyadh", "Asia/Riyadh (AST, UTC+3)"),
        ("Asia/Dubai", "Asia/Dubai (GST, UTC+4)"),
        ("Asia/Tehran", "Asia/Tehran (IRST, UTC+3:30)"),
        ("Asia/Karachi", "Asia/Karachi (PKT, UTC+5)"),
        ("Asia/Kolkata", "Asia/Kolkata (IST, UTC+5:30)"),
        ("Asia/Dhaka", "Asia/Dhaka (BST, UTC+6)"),
        ("Asia/Bangkok", "Asia/Bangkok (ICT, UTC+7)"),
        ("Asia/Jakarta", "Asia/Jakarta (WIB, UTC+7)"),
        ("Asia/Singapore", "Asia/Singapore (SGT, UTC+8)"),
        ("Asia/Hong_Kong", "Asia/Hong_Kong (HKT, UTC+8)"),
        ("Asia/Shanghai", "Asia/Shanghai (CST, UTC+8)"),
        ("Asia/Taipei", "Asia/Taipei (CST, UTC+8)"),
        ("Asia/Manila", "Asia/Manila (PST, UTC+8)"),
        ("Asia/Tokyo", "Asia/Tokyo (JST, UTC+9)"),
        ("Asia/Seoul", "Asia/Seoul (KST, UTC+9)"),
        (
            "Australia/Sydney",
            "Australia/Sydney (AEST/AEDT, UTC+10/+11)",
        ),
        (
            "Australia/Melbourne",
            "Australia/Melbourne (AEST/AEDT, UTC+10/+11)",
        ),
        ("Australia/Brisbane", "Australia/Brisbane (AEST, UTC+10)"),
        (
            "Australia/Adelaide",
            "Australia/Adelaide (ACST/ACDT, UTC+9:30/+10:30)",
        ),
        ("Australia/Perth", "Australia/Perth (AWST, UTC+8)"),
        ("Pacific/Guam", "Pacific/Guam (ChST, UTC+10)"),
        (
            "Pacific/Auckland",
            "Pacific/Auckland (NZST/NZDT, UTC+12/+13)",
        ),
        ("Pacific/Fiji", "Pacific/Fiji (FJT, UTC+12)"),
        ("UTC", "UTC (Coordinated Universal Time)"),
    ];
    let res: Vec<serde_json::Value> = zones
        .iter()
        .map(|(val, lbl)| json!({"value": val, "label": lbl}))
        .collect();
    HttpResponse::Ok().json(res)
}

#[get("/api/hardware")]
async fn get_hardware_compat(data: web::Data<AppState>) -> impl Responder {
    let s = data.config.settings.read();
    HttpResponse::Ok().json(&s.matrix)
}

#[get("/api/gyro")]
async fn get_gyro_compat() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "present": false,
        "pitch": 0.0,
        "roll": 0.0,
        "orientation": 0
    }))
}

#[post("/api/display/orientation")]
async fn post_orientation_compat() -> impl Responder {
    HttpResponse::Ok().json(json!({"status": "ok"}))
}

#[post("/api/gyro/calibrate")]
async fn post_gyro_calibrate_compat() -> impl Responder {
    HttpResponse::Ok().json(json!({"status": "ok"}))
}

// =====================================================================================
// GIF LIBRARY MANAGEMENT (parity with the ESP32 firmware: /api/gifs/*)
//   GET    /api/gifs/library                  -> live scan of the gifs root (folders + counts)
//   POST   /api/gifs/upload?folder=<Name>     -> multipart upload of one or many files
//   POST   /api/gifs/reindex                  -> the RPi scans live; returns the library (no index files needed)
//   DELETE /api/gifs/file?folder=<N>&name=<F> -> delete one file
// =====================================================================================

fn gifs_root() -> std::path::PathBuf {
    find_gif_candidate_roots("yoko")
        .into_iter()
        .find(|p| p.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("gifs"))
}

/// Keep only a safe basename: letters, digits, `_ - space` (and `.` when an extension is allowed).
fn gifs_sanitize_name(input: &str, allow_ext: bool) -> String {
    let base = input.rsplit(|c| c == '/' || c == '\\').next().unwrap_or("");
    let filtered: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == ' ' || (allow_ext && *c == '.'))
        .collect();
    filtered.trim().trim_start_matches('.').chars().take(64).collect()
}

fn gifs_has_media_ext(name: &str) -> bool {
    let l = name.to_lowercase();
    l.ends_with(".gif") || l.ends_with(".png") || l.ends_with(".raw")
}

fn gifs_list_folder(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_file() && !name.starts_with('.') && name != "index.txt" && gifs_has_media_ext(&name) {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

fn gifs_scan_library() -> serde_json::Value {
    let root = gifs_root();
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
            folders.insert(name.clone(), json!({"path": format!("/gifs/{}", name), "count": count}));
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
    // The RPi GIF engine scans folders live, so there is nothing to rebuild; this mirrors the
    // ESP32 endpoint (which rewrites index.txt / playlists.json) and returns the same shape.
    let mut lib = gifs_scan_library();
    if let Some(obj) = lib.as_object_mut() {
        obj.insert("status".to_string(), json!("ok"));
    }
    HttpResponse::Ok().json(lib)
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
    if raw_folder.contains('/') || raw_name.contains('/') || raw_folder.contains("..") || raw_name.contains("..")
        || folder.is_empty() || name.is_empty() || !gifs_has_media_ext(&name) {
        return HttpResponse::BadRequest()
            .json(json!({"status": "error", "message": "folder and name (.gif/.png/.raw) are required"}));
    }
    let path = gifs_root().join(&folder).join(&name);
    if !path.is_file() {
        return HttpResponse::NotFound().json(json!({"status": "error", "message": "File not found"}));
    }
    match std::fs::remove_file(&path) {
        Ok(_) => HttpResponse::Ok().json(json!({"status": "ok", "deleted": format!("/gifs/{}/{}", folder, name)})),
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
        return HttpResponse::BadRequest().json(json!({"status": "error", "message": "folder is required"}));
    }
    let dir = gifs_root().join(&folder);
    if !dir.is_dir() {
        return HttpResponse::NotFound().json(json!({"status": "error", "message": "Folder not found"}));
    }
    let mut files = Vec::new();
    for name in gifs_list_folder(&dir) {
        let size = std::fs::metadata(dir.join(&name)).map(|m| m.len()).unwrap_or(0);
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
        return HttpResponse::BadRequest().json(json!({"status": "error", "message": "folder is required"}));
    }
    let dir = gifs_root().join(&folder);
    if !dir.is_dir() {
        return HttpResponse::NotFound().json(json!({"status": "error", "message": "Folder not found"}));
    }
    // count what we remove (whole playlist folder incl. subfolders such as Logo/256)
    let removed = walkdir_count(&dir);
    match std::fs::remove_dir_all(&dir) {
        Ok(_) => HttpResponse::Ok().json(json!({"status": "ok", "deleted": format!("/gifs/{}", folder), "files": removed})),
        Err(e) => HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("Delete failed: {}", e)})),
    }
}

fn walkdir_count(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { n += walkdir_count(&p); } else { n += 1; }
        }
    }
    n
}

fn gifs_bad(raw: &str) -> bool { raw.contains('/') || raw.contains('\\') || raw.contains("..") }

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
        return HttpResponse::BadRequest().json(json!({"status": "error", "message": "folder must be a plain name"}));
    }
    let dir = gifs_root().join(&folder);
    if dir.exists() {
        return HttpResponse::Conflict().json(json!({"status": "error", "message": "Folder already exists"}));
    }
    match std::fs::create_dir_all(&dir) {
        Ok(_) => HttpResponse::Ok().json(json!({"status": "ok", "folder": folder, "path": format!("/gifs/{}", folder)})),
        Err(e) => HttpResponse::InternalServerError().json(json!({"status": "error", "message": format!("{}", e)})),
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
    if gifs_bad(raw_folder) || gifs_bad(raw_name) || gifs_bad(raw_to) || folder.is_empty() || raw_to.is_empty() {
        return HttpResponse::BadRequest().json(json!({"status": "error", "message": "folder and to are required (plain names)"}));
    }
    let root = gifs_root();
    let (from, to, what) = if raw_name.is_empty() {
        let to_folder = gifs_sanitize_name(raw_to, false);
        if to_folder.is_empty() { return HttpResponse::BadRequest().json(json!({"status": "error", "message": "bad target name"})); }
        (root.join(&folder), root.join(&to_folder), format!("/gifs/{}", to_folder))
    } else {
        let name = gifs_sanitize_name(raw_name, true);
        let mut to_name = gifs_sanitize_name(raw_to, true);
        if !gifs_has_media_ext(&to_name) {
            // keep the original extension if the user typed a bare name
            if let Some(ext) = std::path::Path::new(&name).extension().and_then(|e| e.to_str()) { to_name = format!("{}.{}", to_name, ext); }
        }
        if name.is_empty() || to_name.is_empty() || !gifs_has_media_ext(&to_name) {
            return HttpResponse::BadRequest().json(json!({"status": "error", "message": "bad file name"}));
        }
        (root.join(&folder).join(&name), root.join(&folder).join(&to_name), format!("/gifs/{}/{}", folder, to_name))
    };
    if !from.exists() {
        return HttpResponse::NotFound().json(json!({"status": "error", "message": "Source not found"}));
    }
    if to.exists() {
        return HttpResponse::Conflict().json(json!({"status": "error", "message": "Target already exists"}));
    }
    match std::fs::rename(&from, &to) {
        Ok(_) => HttpResponse::Ok().json(json!({"status": "ok", "renamed_to": what})),
        Err(e) => HttpResponse::InternalServerError().json(json!({"status": "error", "message": format!("{}", e)})),
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
    if gifs_bad(raw_folder) || gifs_bad(raw_name) || folder.is_empty() || name.is_empty() || !gifs_has_media_ext(&name) {
        return HttpResponse::BadRequest().json(json!({"status": "error", "message": "folder and name are required"}));
    }
    let path = gifs_root().join(&folder).join(&name);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
            let mut resp = HttpResponse::Ok();
            resp.content_type(mime);
            resp.insert_header(("Cache-Control", "no-cache"));
            if query.get("download").map(|v| v == "1").unwrap_or(false) {
                resp.insert_header(("Content-Disposition", format!("attachment; filename=\"{}\"", name)));
            }
            resp.body(bytes)
        }
        Err(_) => HttpResponse::NotFound().json(json!({"status": "error", "message": "File not found"})),
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
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error", "message": format!("Failed to create folder: {}", e)}));
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

#[derive(RustEmbed)]
#[folder = "api/www/"]
struct WebAssets;

async fn serve_static(req: actix_web::HttpRequest) -> impl actix_web::Responder {
    let mut p = req.path().trim_start_matches('/').to_string();
    if p.is_empty() {
        p = "index.html".to_string();
    }

    // Check disk first for real-time web development updates
    let disk_path = Path::new("api/www").join(&p);
    if disk_path.exists() && disk_path.is_file() {
        if let Ok(bytes) = std::fs::read(&disk_path) {
            let mime = mime_guess::from_path(&disk_path).first_or_octet_stream();
            return actix_web::HttpResponse::Ok()
                .insert_header(("Cache-Control", "no-cache, no-store, must-revalidate"))
                .content_type(mime.as_ref())
                .body(bytes);
        }
    }

    match WebAssets::get(&p) {
        Some(content) => {
            let mime = mime_guess::from_path(&p).first_or_octet_stream();
            actix_web::HttpResponse::Ok()
                .insert_header(("Cache-Control", "no-cache, no-store, must-revalidate"))
                .content_type(mime.as_ref())
                .body(content.data.into_owned())
        }
        None => actix_web::HttpResponse::NotFound().body("404 Not Found"),
    }
}

pub async fn run_server(config: Arc<Config>, port: u16) -> std::io::Result<()> {
    let app_state = web::Data::new(AppState { config });
    HttpServer::new(move || {
        let cors = actix_cors::Cors::permissive();
        App::new()
            .wrap(cors)
            .app_data(app_state.clone())
            .service(get_system)
            .service(post_system)
            .service(get_hardware_compat)
            .service(get_gyro_compat)
            .service(post_orientation_compat)
            .service(post_gyro_calibrate_compat)
            .service(get_instances)
            .service(post_instances)
            .service(delete_instance)
            .service(get_rotation)
            .service(post_rotation)
            .service(get_engines)
            .service(reboot)
            .service(post_reboot)
            .service(post_restart)
            .service(post_shutdown)
            .service(post_power)
            .service(get_stats)
            .service(get_fonts)
            .service(get_playlists)
            .service(get_gifs_playlists)
            .service(get_themes)
            .service(get_timezones)
            .service(post_wifi)
            .service(post_marquee)
            .service(post_upload)
            .service(get_gifs_library)
            .service(post_gifs_upload)
            .service(post_gifs_reindex)
            .service(delete_gifs_file)
            .service(get_gifs_files)
            .service(delete_gifs_folder)
            .service(post_gifs_mkdir)
            .service(post_gifs_rename)
            .service(get_gifs_file)
            .service(post_mqtt_install)
            .service(post_mqtt_logs)
            .service(crate::api::ota::get_version)
            .service(crate::api::ota::check_update)
            .service(crate::api::ota::auto_update)
            .service(crate::api::ota::handle_update)
            .route("/", web::get().to(serve_static))
            .route("/{_:.*}", web::get().to(serve_static))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
