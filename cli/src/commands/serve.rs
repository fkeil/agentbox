use agentbox_core::{
    down_box, list_boxes, list_cache_images, remove_cache_image, BoxInfo, CacheImage,
    ContainerStatus,
};
use axum::{
    extract::Path,
    http::{Method, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use clap::Args;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

#[derive(Args)]
pub struct ServeArgs {
    /// Port to listen on
    #[arg(long, short, default_value = "7070")]
    pub port: u16,
    /// Host to bind to
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid address: {e}"))?;

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers(Any)
        .allow_origin(Any);

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/api/boxes", get(list_boxes_handler))
        .route("/api/boxes/{name}", delete(delete_box_handler))
        .route("/api/agents", get(list_agents_handler))
        .route("/api/images", get(list_images_handler))
        .route("/api/images/{id}", delete(delete_image_handler))
        .layer(cors);

    println!("agentbox serving on http://{addr}");
    println!("  GET  /api/boxes        — list running boxes");
    println!("  DELETE /api/boxes/:name — stop + remove a box");
    println!("  GET  /api/agents       — list available agents");
    println!("  GET  /api/images       — list cache images");
    println!("  DELETE /api/images/:id — remove a cache image");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn root_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn list_boxes_handler() -> Response {
    match list_boxes().await {
        Ok(boxes) => {
            let items: Vec<Value> = boxes.iter().map(box_info_to_json).collect();
            Json(json!({ "boxes": items })).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn delete_box_handler(Path(box_name): Path<String>) -> Response {
    match down_box(&box_name).await {
        Ok(()) => Json(json!({ "deleted": box_name })).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                error_response(StatusCode::NOT_FOUND, &msg)
            } else {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &msg)
            }
        }
    }
}

async fn list_agents_handler() -> Response {
    let manifests_dir = agentbox_core::find_manifests_dir_pub();
    let builtins: &[(&str, &str)] = &[("claude-code", "Claude Code"), ("opencode", "OpenCode")];

    let manifest_agents: Vec<(String, String)> = manifests_dir
        .as_deref()
        .map(agentbox_core::manifest::list_manifests)
        .unwrap_or_default();

    let manifest_ids: std::collections::HashSet<&str> =
        manifest_agents.iter().map(|(id, _)| id.as_str()).collect();

    let mut items: Vec<Value> = manifest_agents
        .iter()
        .map(|(id, name)| json!({ "id": id, "name": name, "source": "manifest" }))
        .collect();

    for (id, name) in builtins {
        if !manifest_ids.contains(*id) {
            items.push(json!({ "id": id, "name": name, "source": "built-in" }));
        }
    }

    Json(json!({ "agents": items })).into_response()
}

async fn list_images_handler() -> Response {
    match list_cache_images().await {
        Ok(images) => {
            let items: Vec<Value> = images.iter().map(cache_image_to_json).collect();
            Json(json!({ "images": items })).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn delete_image_handler(Path(id): Path<String>) -> Response {
    match remove_cache_image(&id).await {
        Ok(()) => Json(json!({ "deleted": id })).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                error_response(StatusCode::NOT_FOUND, &msg)
            } else {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &msg)
            }
        }
    }
}

fn box_info_to_json(b: &BoxInfo) -> Value {
    let status = match b.status {
        ContainerStatus::Running => "running",
        ContainerStatus::Stopped => "stopped",
    };
    json!({
        "name": b.box_name,
        "id": b.container_id,
        "agent": b.agent_display_name,
        "status": status,
        "folder": b.folder,
        "lifecycle": b.lifecycle,
    })
}

fn cache_image_to_json(img: &CacheImage) -> Value {
    json!({
        "id": img.agent_id,
        "tag": img.image_name,
        "size_mb": img.size_mb,
        "created": img.created_unix,
    })
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    let mut r = Json(json!({ "error": msg })).into_response();
    *r.status_mut() = status;
    r
}

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>agentbox</title>
<style>
  :root { --bg:#0f1117; --surface:#1a1d27; --border:#2a2d3a; --accent:#5b8af8; --text:#e2e4f0; --muted:#6b7280; --red:#ef4444; --green:#22c55e; }
  * { box-sizing:border-box; margin:0; padding:0; }
  body { background:var(--bg); color:var(--text); font-family:system-ui,sans-serif; min-height:100vh; }
  header { background:var(--surface); border-bottom:1px solid var(--border); padding:1rem 2rem; display:flex; align-items:center; gap:1rem; }
  header h1 { font-size:1.25rem; font-weight:700; color:var(--accent); }
  header span { color:var(--muted); font-size:.875rem; }
  main { padding:2rem; max-width:1100px; margin:0 auto; }
  h2 { font-size:1rem; font-weight:600; color:var(--muted); text-transform:uppercase; letter-spacing:.05em; margin-bottom:1rem; margin-top:2rem; }
  .card { background:var(--surface); border:1px solid var(--border); border-radius:.5rem; padding:1rem 1.25rem; margin-bottom:.75rem; display:flex; align-items:center; gap:1rem; }
  .card-name { font-weight:600; flex:1; }
  .card-meta { color:var(--muted); font-size:.8rem; }
  .badge { display:inline-block; padding:.15rem .5rem; border-radius:.25rem; font-size:.75rem; font-weight:600; }
  .badge-running { background:#166534; color:var(--green); }
  .badge-stopped { background:#3f1212; color:var(--red); }
  .badge-unknown { background:#2a2d3a; color:var(--muted); }
  .btn { background:transparent; border:1px solid var(--border); color:var(--text); padding:.35rem .75rem; border-radius:.375rem; cursor:pointer; font-size:.8rem; }
  .btn:hover { border-color:var(--red); color:var(--red); }
  .empty { color:var(--muted); font-size:.875rem; padding:1rem 0; }
  #status { position:fixed; bottom:1rem; right:1rem; background:var(--surface); border:1px solid var(--border); border-radius:.5rem; padding:.5rem 1rem; font-size:.8rem; display:none; }
</style>
</head>
<body>
<header>
  <h1>agentbox</h1>
  <span id="hdr-sub">loading…</span>
</header>
<main>
  <h2>Running Boxes</h2>
  <div id="boxes-list"></div>
  <h2>Agents</h2>
  <div id="agents-list"></div>
  <h2>Cache Images</h2>
  <div id="images-list"></div>
</main>
<div id="status"></div>
<script>
const api = async (method, path) => {
  const r = await fetch(path, { method });
  return r.json();
};
const toast = (msg, ok=true) => {
  const el = document.getElementById('status');
  el.textContent = msg;
  el.style.display = 'block';
  el.style.color = ok ? 'var(--green)' : 'var(--red)';
  setTimeout(() => el.style.display='none', 3000);
};
const badgeClass = s => s==='running'?'badge-running':s==='stopped'?'badge-stopped':'badge-unknown';

async function loadBoxes() {
  const { boxes = [] } = await api('GET', '/api/boxes');
  const el = document.getElementById('boxes-list');
  document.getElementById('hdr-sub').textContent = `${boxes.length} box${boxes.length!==1?'es':''}`;
  if (!boxes.length) { el.innerHTML = '<p class="empty">No boxes found.</p>'; return; }
  el.innerHTML = boxes.map(b => `
    <div class="card">
      <div>
        <div class="card-name">${b.name || b.id.slice(0,12)}</div>
        <div class="card-meta">${b.agent || ''} &nbsp;·&nbsp; ${b.image || ''}</div>
      </div>
      <span class="badge ${badgeClass(b.status)}">${b.status}</span>
      <button class="btn" onclick="deleteBox('${b.name || b.id}')">Stop &amp; Remove</button>
    </div>`).join('');
}

async function deleteBox(name) {
  if (!confirm(`Stop and remove box "${name}"?`)) return;
  const r = await api('DELETE', `/api/boxes/${name}`);
  if (r.error) { toast('Error: ' + r.error, false); } else { toast(`Box "${name}" removed.`); loadBoxes(); }
}

async function loadAgents() {
  const { agents = [] } = await api('GET', '/api/agents');
  const el = document.getElementById('agents-list');
  if (!agents.length) { el.innerHTML = '<p class="empty">No agents found.</p>'; return; }
  el.innerHTML = agents.map(a => `
    <div class="card">
      <div class="card-name">${a.name}</div>
      <div class="card-meta">${a.id}</div>
    </div>`).join('');
}

async function loadImages() {
  const { images = [] } = await api('GET', '/api/images');
  const el = document.getElementById('images-list');
  if (!images.length) { el.innerHTML = '<p class="empty">No cache images.</p>'; return; }
  el.innerHTML = images.map(img => `
    <div class="card">
      <div>
        <div class="card-name">${img.tag}</div>
        <div class="card-meta">${img.size_mb} MiB</div>
      </div>
      <button class="btn" onclick="deleteImage('${img.id}')">Remove</button>
    </div>`).join('');
}

async function deleteImage(id) {
  const r = await api('DELETE', `/api/images/${id}`);
  if (r.error) { toast('Error: ' + r.error, false); } else { toast('Image removed.'); loadImages(); }
}

loadBoxes(); loadAgents(); loadImages();
setInterval(loadBoxes, 5000);
</script>
</body>
</html>"#;
