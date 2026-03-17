use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Section>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trashed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<Permission>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Section {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<ListContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_items: Option<Vec<ListItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_list_items: Option<Vec<ListItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Group>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<Family>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Role {
    RoleUnspecified,
    Owner,
    Writer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Family {}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNotesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<Note>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_serialization_skips_none_fields() {
        let note = Note {
            title: Some("Test".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&note).unwrap();
        assert!(json.contains("\"title\":\"Test\""));
        assert!(!json.contains("name"));
        assert!(!json.contains("body"));
    }

    #[test]
    fn note_deserialization_from_api_response() {
        let json = r#"{
            "name": "notes/abc123",
            "title": "Shopping List",
            "body": {
                "list": {
                    "listItems": [
                        {"text": {"text": "Milk"}, "checked": false},
                        {"text": {"text": "Eggs"}, "checked": true}
                    ]
                }
            },
            "createTime": "2024-01-01T00:00:00Z",
            "updateTime": "2024-01-02T00:00:00Z",
            "trashed": false
        }"#;
        let note: Note = serde_json::from_str(json).unwrap();
        assert_eq!(note.name.unwrap(), "notes/abc123");
        assert_eq!(note.title.unwrap(), "Shopping List");
        let list = note.body.unwrap().list.unwrap();
        let items = list.list_items.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text.as_ref().unwrap().text.as_ref().unwrap(), "Milk");
        assert_eq!(items[1].checked, Some(true));
    }

    #[test]
    fn text_note_round_trip() {
        let note = Note {
            title: Some("My Note".into()),
            body: Some(Section {
                text: Some(TextContent {
                    text: Some("Hello world".into()),
                }),
                list: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&note).unwrap();
        let parsed: Note = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title.unwrap(), "My Note");
        assert_eq!(
            parsed.body.unwrap().text.unwrap().text.unwrap(),
            "Hello world"
        );
    }

    #[test]
    fn list_notes_response_deserialization() {
        let json = r#"{
            "notes": [{"name": "notes/1", "title": "A"}],
            "nextPageToken": "token123"
        }"#;
        let resp: ListNotesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.notes.unwrap().len(), 1);
        assert_eq!(resp.next_page_token.unwrap(), "token123");
    }

    #[test]
    fn empty_list_notes_response() {
        let json = r#"{}"#;
        let resp: ListNotesResponse = serde_json::from_str(json).unwrap();
        assert!(resp.notes.is_none());
        assert!(resp.next_page_token.is_none());
    }

    #[test]
    fn role_serialization() {
        let json = serde_json::to_string(&Role::Owner).unwrap();
        assert_eq!(json, "\"OWNER\"");
        let json = serde_json::to_string(&Role::Writer).unwrap();
        assert_eq!(json, "\"WRITER\"");
    }
}
