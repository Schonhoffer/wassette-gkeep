mod types;

#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    world: "gkeep",
    exports: {
        world: Component,
    },
});

#[cfg(target_arch = "wasm32")]
use spin_sdk::http::{send, Request, Response};
#[cfg(target_arch = "wasm32")]
use types::*;

#[cfg(target_arch = "wasm32")]
const BASE_URL: &str = "https://keep.googleapis.com/v1";

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
fn token() -> Result<String, String> {
    std::env::var("GOOGLE_KEEP_TOKEN")
        .map_err(|_| "GOOGLE_KEEP_TOKEN environment variable not set".to_string())
}

#[cfg(target_arch = "wasm32")]
fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(target_arch = "wasm32")]
fn validate_note_id(note_id: &str) -> Result<&str, String> {
    let id = note_id.strip_prefix("notes/").unwrap_or(note_id);
    if id.is_empty() || !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("Invalid note ID: {}", note_id));
    }
    Ok(id)
}

#[cfg(target_arch = "wasm32")]
fn auth_get(url: &str, token: &str) -> Request {
    Request::builder()
        .method(spin_sdk::http::Method::Get)
        .uri(url)
        .header("Authorization", format!("Bearer {}", token))
        .build()
}

#[cfg(target_arch = "wasm32")]
fn auth_post(url: &str, token: &str, body: &str) -> Request {
    Request::builder()
        .method(spin_sdk::http::Method::Post)
        .uri(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(body)
        .build()
}

#[cfg(target_arch = "wasm32")]
fn auth_patch(url: &str, token: &str, body: &str) -> Request {
    Request::builder()
        .method(spin_sdk::http::Method::Patch)
        .uri(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(body)
        .build()
}

#[cfg(target_arch = "wasm32")]
fn auth_delete(url: &str, token: &str) -> Request {
    Request::builder()
        .method(spin_sdk::http::Method::Delete)
        .uri(url)
        .header("Authorization", format!("Bearer {}", token))
        .build()
}

#[cfg(target_arch = "wasm32")]
fn check(response: &Response) -> Result<(), String> {
    let status = *response.status();
    if !(200..300).contains(&status) {
        let body = String::from_utf8_lossy(response.body());
        return Err(format!("API error ({}): {}", status, body));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn body_string(response: &Response) -> String {
    String::from_utf8_lossy(response.body()).into_owned()
}

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn list_notes(filter: String, page_size: u32, page_token: String) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let mut url = format!("{}/notes", BASE_URL);
            let mut params = Vec::new();
            if page_size > 0 {
                params.push(format!("pageSize={}", page_size));
            }
            if !page_token.is_empty() {
                params.push(format!("pageToken={}", url_encode(&page_token)));
            }
            if !filter.is_empty() {
                params.push(format!("filter={}", url_encode(&filter)));
            }
            if !params.is_empty() {
                url.push('?');
                url.push_str(&params.join("&"));
            }
            let resp = send(auth_get(&url, &token)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }

    fn get_note(note_id: String) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let id = validate_note_id(&note_id)?;
            let url = format!("{}/notes/{}", BASE_URL, id);
            let resp = send(auth_get(&url, &token)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }

    fn create_text_note(title: String, body: String) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let note = Note {
                title: Some(title),
                body: Some(Section {
                    text: Some(TextContent { text: Some(body) }),
                    list: None,
                }),
                ..Default::default()
            };
            let json = serde_json::to_string(&note).map_err(|e| e.to_string())?;
            let url = format!("{}/notes", BASE_URL);
            let resp = send(auth_post(&url, &token, &json)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }

    fn create_list_note(title: String, items_json: String) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let list_items: Vec<ListItem> = serde_json::from_str(&items_json)
                .map_err(|e| format!("Invalid items JSON: {}", e))?;
            let note = Note {
                title: Some(title),
                body: Some(Section {
                    text: None,
                    list: Some(ListContent {
                        list_items: Some(list_items),
                    }),
                }),
                ..Default::default()
            };
            let json = serde_json::to_string(&note).map_err(|e| e.to_string())?;
            let url = format!("{}/notes", BASE_URL);
            let resp = send(auth_post(&url, &token, &json)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }

    fn delete_note(note_id: String) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let id = validate_note_id(&note_id)?;
            let url = format!("{}/notes/{}", BASE_URL, id);
            let resp = send(auth_delete(&url, &token)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }

    fn update_note(note_id: String, content_json: String, update_mask: String) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let id = validate_note_id(&note_id)?;
            let mut url = format!("{}/notes/{}", BASE_URL, id);
            if !update_mask.is_empty() {
                url.push_str(&format!("?updateMask={}", url_encode(&update_mask)));
            }
            let resp = send(auth_patch(&url, &token, &content_json)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }

    fn add_list_item(note_id: String, text: String, checked: bool) -> Result<String, String> {
        spin_executor::run(async {
            let token = token()?;
            let id = validate_note_id(&note_id)?;
            let url = format!("{}/notes/{}", BASE_URL, id);

            let resp = send(auth_get(&url, &token)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            let mut note: Note = serde_json::from_str(&body_string(&resp))
                .map_err(|e| format!("Failed to parse note: {}", e))?;

            let list = note.body.as_mut()
                .and_then(|b| b.list.as_mut())
                .ok_or_else(|| "Note is not a list".to_string())?;

            let new_item = ListItem {
                text: Some(TextContent { text: Some(text) }),
                checked: Some(checked),
                child_list_items: None,
            };

            match list.list_items.as_mut() {
                Some(items) => items.insert(0, new_item),
                None => list.list_items = Some(vec![new_item]),
            }

            let patch = Note {
                body: note.body,
                ..Default::default()
            };
            let json = serde_json::to_string(&patch).map_err(|e| e.to_string())?;
            let patch_url = format!("{}?updateMask={}", url, url_encode("body.list.listItems"));
            let resp = send(auth_patch(&patch_url, &token, &json)).await.map_err(|e| e.to_string())?;
            check(&resp)?;
            Ok(body_string(&resp))
        })
    }
}

