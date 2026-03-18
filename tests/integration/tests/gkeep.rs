//! Integration tests for wassette-gkeep.
//!
//! These tests compile the WASM component, start a mock Google Keep API server,
//! launch `wassette` with the component, and exercise each MCP tool end-to-end.

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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{timeout, Duration};

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

fn wassette_available() -> bool {
    std::process::Command::new("wassette")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

// ── MCP client over stdio ───────────────────────────────────────────

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

const MCP_TIMEOUT: Duration = Duration::from_secs(30);

impl McpClient {
    async fn start(wasm_path: &std::path::Path, mock_port: u16) -> Self {
        let base_url = format!("http://127.0.0.1:{}/v1", mock_port);
        let net_allow = format!("127.0.0.1:{}", mock_port);

        let mut child = Command::new("wassette")
            .args([
                "serve",
                "--stdio",
                "--load",
                wasm_path.to_str().unwrap(),
                "--env",
                "GOOGLE_KEEP_TOKEN",
                "--env",
                "GKEEP_API_BASE_URL",
                "--net-allow",
                &net_allow,
            ])
            .env("GOOGLE_KEEP_TOKEN", "test-token-12345")
            .env("GKEEP_API_BASE_URL", &base_url)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to start wassette — is it installed?");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        McpClient {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    async fn send_message(&mut self, msg: &Value) {
        let json = serde_json::to_vec(msg).unwrap();
        let header = format!("Content-Length: {}\r\n\r\n", json.len());
        self.stdin.write_all(header.as_bytes()).await.unwrap();
        self.stdin.write_all(&json).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn read_message(&mut self) -> Value {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).await.unwrap();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(len_str.trim().parse().unwrap());
            }
        }
        let len = content_length.expect("Missing Content-Length header");
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).await.unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    async fn read_response(&mut self, expected_id: u64) -> Value {
        loop {
            let msg = timeout(MCP_TIMEOUT, self.read_message())
                .await
                .expect("Timed out waiting for MCP response");
            if msg.get("id").and_then(|v| v.as_u64()) == Some(expected_id) {
                return msg;
            }
        }
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
        self.send_message(&msg).await;
        self.read_response(id).await
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
        self.send_message(&json!({
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
    fn tool_text(response: &Value) -> String {
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

// ── Test fixture ────────────────────────────────────────────────────

struct TestFixture {
    client: McpClient,
    state: MockState,
    _mock_handle: tokio::task::JoinHandle<()>,
}

async fn setup(initial_notes: Vec<(&str, Value)>) -> TestFixture {
    if !wassette_available() {
        panic!("wassette CLI not found — install it to run integration tests");
    }

    let wasm = ensure_wasm_built();
    let state = MockState::new();
    for (id, note) in &initial_notes {
        state.seed(id, note.clone());
    }

    let (port, mock_handle) = start_mock(state.clone()).await;
    // Give the mock listener a moment to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut client = McpClient::start(&wasm, port).await;
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
    assert_eq!(parsed["notes"].as_array().unwrap().len(), 0);
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
