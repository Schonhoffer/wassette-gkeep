//! Integration tests for wassette-gkeep.
//!
//! These tests compile the WASM component, download the wassette binary (if
//! needed), start a mock Google Keep API server, launch `wassette serve
//! --streamable-http` with the component, and exercise each MCP tool over HTTP.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::time::Duration;

// ── Configuration ───────────────────────────────────────────────────

const WASSETTE_VERSION: &str = "0.4.0";
const COMPONENT_ID: &str = "wassette_gkeep";
const MCP_TIMEOUT: Duration = Duration::from_secs(30);

// ── Workspace helpers ───────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn ensure_wasm_built() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = workspace_root();
        eprintln!("Building WASM component…");
        let output = std::process::Command::new("cargo")
            .args([
                "build",
                "--release",
                "--target",
                "wasm32-wasip2",
                "-p",
                "wassette-gkeep",
            ])
            .current_dir(&root)
            .output()
            .expect("Failed to run cargo build");
        assert!(
            output.status.success(),
            "WASM build failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wasm = root.join("target/wasm32-wasip2/release/wassette_gkeep.wasm");
        assert!(wasm.exists(), "WASM not found at {}", wasm.display());
        wasm
    })
    .clone()
}

// ── Wassette binary management ──────────────────────────────────────

fn ensure_wassette() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let bin_dir = workspace_root().join("target").join("wassette");
        std::fs::create_dir_all(&bin_dir).unwrap();

        let binary_name = if cfg!(windows) {
            "wassette.exe"
        } else {
            "wassette"
        };
        let binary_path = bin_dir.join(binary_name);

        if binary_path.exists() {
            if let Ok(out) = std::process::Command::new(&binary_path)
                .arg("--version")
                .output()
            {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.contains(WASSETTE_VERSION) {
                    eprintln!("Using cached wassette v{}", WASSETTE_VERSION);
                    return binary_path;
                }
            }
            eprintln!("Cached wassette has wrong version, re-downloading…");
        }

        eprintln!("Downloading wassette v{}…", WASSETTE_VERSION);
        download_wassette(&bin_dir);
        assert!(
            binary_path.exists(),
            "wassette binary not found at {} after download",
            binary_path.display()
        );
        binary_path
    })
    .clone()
}

#[cfg(target_os = "windows")]
fn download_wassette(bin_dir: &std::path::Path) {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    let url = format!(
        "https://github.com/microsoft/wassette/releases/download/v{ver}/wassette_{ver}_windows_{arch}.zip",
        ver = WASSETTE_VERSION,
        arch = arch,
    );
    let zip_path = bin_dir.join("wassette.zip");

    let status = std::process::Command::new("curl")
        .args(["-fsSL", "-o", zip_path.to_str().unwrap(), &url])
        .status()
        .expect("curl not found — install curl or add it to PATH");
    assert!(status.success(), "Failed to download wassette from {}", url);

    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                zip_path.display(),
                bin_dir.display()
            ),
        ])
        .status()
        .expect("PowerShell not found");
    assert!(status.success(), "Failed to extract wassette zip");

    let _ = std::fs::remove_file(&zip_path);
}

#[cfg(not(target_os = "windows"))]
fn download_wassette(bin_dir: &std::path::Path) {
    let (os, arch) = if cfg!(target_os = "macos") {
        (
            "darwin",
            if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "amd64"
            },
        )
    } else {
        (
            "linux",
            if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "amd64"
            },
        )
    };
    let url = format!(
        "https://github.com/microsoft/wassette/releases/download/v{ver}/wassette_{ver}_{os}_{arch}.tar.gz",
        ver = WASSETTE_VERSION,
        os = os,
        arch = arch,
    );

    let status = std::process::Command::new("sh")
        .args([
            "-c",
            &format!("curl -fsSL '{}' | tar -xz -C '{}'", url, bin_dir.display()),
        ])
        .status()
        .expect("Failed to run curl | tar");
    assert!(status.success(), "Failed to download wassette from {}", url);
}

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// ── Mock Google Keep API ────────────────────────────────────────────

#[derive(Clone, Default)]
struct MockState {
    notes: Arc<Mutex<HashMap<String, Value>>>,
    next_id: Arc<Mutex<u32>>,
}

impl MockState {
    fn new() -> Self {
        Self {
            notes: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    fn seed(&self, id: &str, note: Value) {
        self.notes.lock().unwrap().insert(id.to_string(), note);
    }

    fn alloc_id(&self) -> String {
        let mut next = self.next_id.lock().unwrap();
        let id = format!("note{}", *next);
        *next += 1;
        id
    }
}

fn verify_auth(headers: &HeaderMap) -> Result<(), StatusCode> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !auth.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

async fn handle_list_notes(
    State(state): State<MockState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    verify_auth(&headers)?;
    let notes = state.notes.lock().unwrap();
    let list: Vec<&Value> = notes.values().collect();
    Ok(Json(json!({ "notes": list })))
}

async fn handle_get_note(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    verify_auth(&headers)?;
    let notes = state.notes.lock().unwrap();
    notes
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn handle_create_note(
    State(state): State<MockState>,
    headers: HeaderMap,
    body: String,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    verify_auth(&headers)?;
    let mut note: Value = serde_json::from_str(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let id = state.alloc_id();
    note["name"] = json!(format!("notes/{}", id));
    state.notes.lock().unwrap().insert(id, note.clone());
    Ok((StatusCode::OK, Json(note)))
}

async fn handle_delete_note(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    verify_auth(&headers)?;
    state
        .notes
        .lock()
        .unwrap()
        .remove(&id)
        .map(|_| Json(json!({})))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn handle_update_note(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Value>, StatusCode> {
    verify_auth(&headers)?;
    let patch: Value = serde_json::from_str(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut notes = state.notes.lock().unwrap();
    let note = notes.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    if let (Some(existing), Some(patch_obj)) = (note.as_object_mut(), patch.as_object()) {
        for (k, v) in patch_obj {
            existing.insert(k.clone(), v.clone());
        }
    }
    Ok(Json(note.clone()))
}

fn mock_router(state: MockState) -> Router {
    Router::new()
        .route("/v1/notes", get(handle_list_notes).post(handle_create_note))
        .route(
            "/v1/notes/{id}",
            get(handle_get_note)
                .patch(handle_update_note)
                .delete(handle_delete_note),
        )
        .with_state(state)
}

async fn start_mock(state: MockState) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = mock_router(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (port, handle)
}

// ── MCP client over streamable HTTP ─────────────────────────────────

struct McpClient {
    child: Child,
    http: reqwest::Client,
    mcp_url: String,
    session_id: Option<String>,
    next_id: u64,
    component_dir: PathBuf,
}

impl McpClient {
    async fn start(
        wassette: &std::path::Path,
        wasm_path: &std::path::Path,
        mock_port: u16,
    ) -> Self {
        let base_url = format!("http://127.0.0.1:{}/v1", mock_port);
        let net_host = format!("127.0.0.1:{}", mock_port);
        let mcp_port = find_free_port();

        // Create an isolated component directory for this test run
        let component_dir =
            std::env::temp_dir().join(format!("wassette-e2e-{}-{}", std::process::id(), mcp_port,));
        std::fs::create_dir_all(&component_dir).expect("Failed to create temp component dir");
        let dir_str = component_dir.to_str().unwrap();

        // Load the WASM component
        let wasm_uri = format!("file://{}", wasm_path.display());
        let out = std::process::Command::new(wassette)
            .args(["component", "load", "--component-dir", dir_str, &wasm_uri])
            .output()
            .expect("Failed to run `wassette component load`");
        assert!(
            out.status.success(),
            "wassette component load failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Grant environment-variable permissions
        for key in &["GOOGLE_KEEP_TOKEN", "GKEEP_API_BASE_URL"] {
            let out = std::process::Command::new(wassette)
                .args([
                    "permission",
                    "grant",
                    "environment-variable",
                    "--component-dir",
                    dir_str,
                    COMPONENT_ID,
                    key,
                ])
                .output()
                .expect("Failed to grant env-var permission");
            assert!(
                out.status.success(),
                "permission grant env-var {} failed:\n{}",
                key,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        // Grant network permission to reach the mock server
        let out = std::process::Command::new(wassette)
            .args([
                "permission",
                "grant",
                "network",
                "--component-dir",
                dir_str,
                COMPONENT_ID,
                &net_host,
            ])
            .output()
            .expect("Failed to grant network permission");
        assert!(
            out.status.success(),
            "permission grant network failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Start the MCP server with streamable HTTP transport
        let bind_addr = format!("127.0.0.1:{}", mcp_port);
        let child = Command::new(wassette)
            .args([
                "serve",
                "--streamable-http",
                "--bind-address",
                &bind_addr,
                "--component-dir",
                dir_str,
                "--disable-builtin-tools",
                "--env",
                &format!("GOOGLE_KEEP_TOKEN=test-token-12345"),
                "--env",
                &format!("GKEEP_API_BASE_URL={}", base_url),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to start wassette serve");

        let mcp_url = format!("http://127.0.0.1:{}/mcp", mcp_port);
        let health_url = format!("http://127.0.0.1:{}/health", mcp_port);
        let http = reqwest::Client::new();

        // Wait for the server to be ready
        let deadline = tokio::time::Instant::now() + MCP_TIMEOUT;
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "wassette server did not become ready within {:?}",
                    MCP_TIMEOUT
                );
            }
            match http.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }

        McpClient {
            child,
            http,
            mcp_url,
            session_id: None,
            next_id: 1,
            component_dir,
        }
    }

    /// Send a JSON-RPC message and parse the response.
    ///
    /// Handles both `application/json` and `text/event-stream` (SSE) responses as
    /// allowed by the MCP Streamable HTTP transport.
    async fn send_rpc(&mut self, body: &Value) -> Option<Value> {
        let mut req = self
            .http
            .post(&self.mcp_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body);

        if let Some(ref sid) = self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }

        let resp = tokio::time::timeout(MCP_TIMEOUT, req.send())
            .await
            .expect("MCP request timed out")
            .expect("MCP HTTP request failed");

        // Capture session ID if present
        if let Some(sid) = resp.headers().get("mcp-session-id") {
            self.session_id = Some(sid.to_str().unwrap().to_string());
        }

        let status = resp.status();
        if status == reqwest::StatusCode::ACCEPTED {
            return None; // Notification acknowledged
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let text = resp.text().await.unwrap();

        let json_val = if content_type.contains("text/event-stream") {
            // Parse SSE: last `data:` line containing valid JSON
            text.lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                .last()
                .unwrap_or_else(|| panic!("No valid JSON in SSE response:\n{}", text))
        } else {
            serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("Invalid JSON response: {}\n{}", e, text))
        };

        Some(json_val)
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.send_rpc(&msg)
            .await
            .unwrap_or_else(|| panic!("Expected response for {} but got 202", method))
    }

    async fn initialize(&mut self) -> Value {
        let resp = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "integration-test", "version": "0.1.0" }
                }),
            )
            .await;
        // Acknowledge with initialized notification (no response expected)
        self.send_rpc(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await;
        resp
    }

    async fn call_tool(&mut self, name: &str, args: Value) -> Value {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": args
            }),
        )
        .await
    }

    /// Extract the text content from an MCP tool-call response.
    ///
    /// The WIT interface returns `result<string, string>`, so wassette wraps the
    /// value as `{"ok":"<json>"}` or `{"err":"<msg>"}`.  This helper unwraps the
    /// `ok` payload and panics on errors.
    fn tool_text(response: &Value) -> String {
        let raw = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        let wrapper: Value =
            serde_json::from_str(raw).unwrap_or_else(|e| panic!("bad tool text: {e}\n{raw}"));
        if let Some(ok) = wrapper.get("ok").and_then(|v| v.as_str()) {
            ok.to_string()
        } else if let Some(err) = wrapper.get("err").and_then(|v| v.as_str()) {
            panic!("Tool returned error: {}", err)
        } else {
            // Fallback: return the raw text as-is (no result wrapper)
            raw.to_string()
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_dir_all(&self.component_dir);
    }
}

// ── Test fixture ────────────────────────────────────────────────────

struct TestFixture {
    client: McpClient,
    state: MockState,
    _mock_handle: tokio::task::JoinHandle<()>,
}

async fn setup(initial_notes: Vec<(&str, Value)>) -> TestFixture {
    let wassette = ensure_wassette();
    let wasm = ensure_wasm_built();
    let state = MockState::new();
    for (id, note) in &initial_notes {
        state.seed(id, note.clone());
    }

    let (port, mock_handle) = start_mock(state.clone()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut client = McpClient::start(&wassette, &wasm, port).await;
    let init_resp = client.initialize().await;
    assert!(
        init_resp.get("result").is_some(),
        "MCP initialize failed: {:?}",
        init_resp
    );

    TestFixture {
        client,
        state,
        _mock_handle: mock_handle,
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_list_notes_empty() {
    let mut f = setup(vec![]).await;
    let resp = f
        .client
        .call_tool(
            "list-notes",
            json!({
                "filter": "",
                "page-size": 0,
                "page-token": ""
            }),
        )
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).expect("response should be valid JSON");
    let notes_arr = parsed["notes"]
        .as_array()
        .unwrap_or_else(|| panic!("Expected notes array in: {}", text));
    assert_eq!(notes_arr.len(), 0);
}

#[tokio::test]
async fn test_list_notes_with_data() {
    let note = json!({
        "name": "notes/abc123",
        "title": "Shopping List"
    });
    let mut f = setup(vec![("abc123", note)]).await;
    let resp = f
        .client
        .call_tool(
            "list-notes",
            json!({
                "filter": "",
                "page-size": 0,
                "page-token": ""
            }),
        )
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let notes = parsed["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["title"], "Shopping List");
}

#[tokio::test]
async fn test_get_note() {
    let note = json!({
        "name": "notes/note42",
        "title": "My Note",
        "body": {
            "text": { "text": "Hello world" }
        }
    });
    let mut f = setup(vec![("note42", note)]).await;
    let resp = f
        .client
        .call_tool("get-note", json!({ "note-id": "note42" }))
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["title"], "My Note");
    assert_eq!(parsed["body"]["text"]["text"], "Hello world");
}

#[tokio::test]
async fn test_get_note_with_prefix() {
    let note = json!({
        "name": "notes/prefixed1",
        "title": "Prefixed"
    });
    let mut f = setup(vec![("prefixed1", note)]).await;
    let resp = f
        .client
        .call_tool("get-note", json!({ "note-id": "notes/prefixed1" }))
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["title"], "Prefixed");
}

#[tokio::test]
async fn test_create_text_note() {
    let mut f = setup(vec![]).await;
    let resp = f
        .client
        .call_tool(
            "create-text-note",
            json!({
                "title": "New Note",
                "body": "Some content"
            }),
        )
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["title"], "New Note");
    assert!(
        parsed["name"].as_str().unwrap().starts_with("notes/"),
        "created note should have a name"
    );
    // Verify the mock stored the note
    let notes = f.state.notes.lock().unwrap();
    assert_eq!(notes.len(), 1);
}

#[tokio::test]
async fn test_create_list_note() {
    let mut f = setup(vec![]).await;
    let items = json!([
        { "text": { "text": "Item 1" }, "checked": false },
        { "text": { "text": "Item 2" }, "checked": true }
    ]);
    let resp = f
        .client
        .call_tool(
            "create-list-note",
            json!({
                "title": "My Checklist",
                "items-json": serde_json::to_string(&items).unwrap()
            }),
        )
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["title"], "My Checklist");
    assert!(parsed["name"].as_str().unwrap().starts_with("notes/"));
}

#[tokio::test]
async fn test_delete_note() {
    let note = json!({
        "name": "notes/todelete",
        "title": "Delete Me"
    });
    let mut f = setup(vec![("todelete", note)]).await;
    let resp = f
        .client
        .call_tool("delete-note", json!({ "note-id": "todelete" }))
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(parsed.is_object(), "delete should return an object");
    // Verify removed from mock
    let notes = f.state.notes.lock().unwrap();
    assert!(!notes.contains_key("todelete"));
}

#[tokio::test]
async fn test_update_note() {
    let note = json!({
        "name": "notes/toupdate",
        "title": "Old Title",
        "body": { "text": { "text": "Old body" } }
    });
    let mut f = setup(vec![("toupdate", note)]).await;
    let resp = f
        .client
        .call_tool(
            "update-note",
            json!({
                "note-id": "toupdate",
                "content-json": r#"{"title":"New Title"}"#,
                "update-mask": "title"
            }),
        )
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["title"], "New Title");
}

#[tokio::test]
async fn test_add_list_item() {
    let note = json!({
        "name": "notes/listid",
        "title": "Shopping",
        "body": {
            "list": {
                "listItems": [
                    { "text": { "text": "Milk" }, "checked": false }
                ]
            }
        }
    });
    let mut f = setup(vec![("listid", note)]).await;
    let resp = f
        .client
        .call_tool(
            "add-list-item",
            json!({
                "note-id": "listid",
                "text": "Eggs",
                "checked": false
            }),
        )
        .await;
    let text = McpClient::tool_text(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    // The component fetches the note, prepends the new item, then PATCHes back.
    // Our mock merges the body, so the patched note should contain list items.
    let items = parsed["body"]["list"]["listItems"].as_array().unwrap();
    assert!(items.len() >= 2, "should have at least 2 list items");
}
