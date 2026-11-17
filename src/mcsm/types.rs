//! Accurate wire types for the MCSManager panel + daemon APIs we use.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard MCSManager envelope. The panel often returns HTTP 200 while the
/// payload status is 403/400/500, so both layers must be checked.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiEnvelope<T> {
    pub status: i64,
    #[serde(default)]
    pub data: Option<T>,
    #[serde(default)]
    pub time: Option<i64>,
}

/// Credentials returned by POST /api/files/upload.
///
/// Newer panels return addr + password (and optionally remoteMappings), while
/// older panels use uploadAddress. Both shapes are represented here so the
/// daemon URL can be built without guessing.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadPassport {
    pub password: String,
    #[serde(default)]
    pub addr: Option<String>,
    #[serde(default)]
    pub upload_address: Option<String>,
    #[serde(default)]
    pub remote_mappings: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceOperationData {
    #[serde(default)]
    pub instance_uuid: Option<String>,
}

/// Only the fields required to observe an instance's lifecycle are modelled.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDetail {
    #[serde(default)]
    pub instance_uuid: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub status: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveFilesRequest<'a> {
    pub targets: Vec<(&'a str, &'a str)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListData {
    #[serde(default)]
    pub items: Vec<FileEntry>,
    #[serde(default)]
    pub total: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteFilesRequest {
    pub targets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UploadTarget {
    pub upload_dir: String,
    pub uploaded_path: String,
    pub final_path: String,
    pub needs_move: bool,
}

#[derive(Debug, Clone)]
pub struct UploadOutcome {
    pub upload_dir: String,
    pub uploaded_path: String,
    pub final_path: String,
    pub bytes: u64,
}
