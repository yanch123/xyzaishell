//! Passive, local shell-reported records. Never an execution or approval source.
use crate::ssh::{ApiError, SessionRef};
use serde::{Deserialize, Serialize};

pub const MAX_COMMAND_BYTES: usize = 16 * 1024;
pub const MAX_CWD_BYTES: usize = 4 * 1024;
pub const MAX_LOG_ROWS: usize = 50_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Running,
    Completed,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShellLogEntry {
    pub id: String,
    pub sequence: String,
    pub session_id: String,
    pub generation: String,
    pub shell_id: String,
    pub shell_sequence: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Local receipt time, not an assertion about the remote machine's clock.
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub remote_started_at: Option<String>,
    pub command: String,
    /// Physical working directory reported immediately before the top-level command.
    pub cwd: String,
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub redacted: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellLogQuery {
    pub session_id: Option<String>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub contains: Option<String>,
    pub before_seq: Option<String>,
    pub snapshot_max_seq: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellLogPage {
    pub entries: Vec<ShellLogEntry>,
    pub next_before_seq: Option<String>,
    pub snapshot_max_seq: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Json,
    Csv,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellLogExport {
    pub path: String,
    pub records: usize,
    pub bytes: usize,
    pub format: ExportFormat,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Disabled,
    Starting,
    Recording,
    Paused,
    Gap,
    Disconnected,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellLogStatus {
    pub enabled: bool,
    pub available: bool,
    pub plaintext: bool,
    pub revision: String,
    pub state: CaptureState,
    pub session: Option<SessionRef>,
    pub reason: Option<String>,
    pub error: Option<ApiError>,
}
