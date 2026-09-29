use std::collections::HashMap;

pub const CONTROL: &str = "/modal.client.ModalClient/";

pub const ROUTER: &str = "/modal.task_command_router.TaskCommandRouter/";

pub const CREATE_IF_MISSING: i32 = 1;

pub const STATUS_UNSPECIFIED: i32 = 0;
pub const STATUS_SUCCESS: i32 = 1;
pub const STATUS_TIMEOUT: i32 = 4;

pub const NETWORK_OPEN: i32 = 1;
pub const NETWORK_BLOCKED: i32 = 2;

pub const PIPE: i32 = 1;
pub const STDOUT: i32 = 0;
pub const STDERR: i32 = 1;

#[derive(Clone, PartialEq, prost::Message)]
pub struct Empty {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AuthTokenGetResponse {
    #[prost(string, tag = "1")]
    pub token: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AppGetOrCreateRequest {
    #[prost(string, tag = "1")]
    pub app_name: String,
    #[prost(string, tag = "2")]
    pub environment_name: String,
    #[prost(int32, tag = "3")]
    pub object_creation_type: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AppGetOrCreateResponse {
    #[prost(string, tag = "1")]
    pub app_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnvironmentGetOrCreateRequest {
    #[prost(string, tag = "1")]
    pub deployment_name: String,
    #[prost(int32, tag = "2")]
    pub object_creation_type: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnvironmentGetOrCreateResponse {
    #[prost(string, tag = "1")]
    pub environment_id: String,
    #[prost(message, optional, tag = "2")]
    pub metadata: Option<EnvironmentMetadata>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnvironmentMetadata {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, tag = "2")]
    pub settings: Option<EnvironmentSettings>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnvironmentSettings {
    #[prost(string, tag = "1")]
    pub image_builder_version: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GenericResult {
    #[prost(int32, tag = "1")]
    pub status: i32,
    #[prost(string, tag = "2")]
    pub exception: String,
    #[prost(int32, tag = "3")]
    pub exitcode: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Image {
    #[prost(string, repeated, tag = "6")]
    pub dockerfile_commands: Vec<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImageGetOrCreateRequest {
    #[prost(message, optional, tag = "2")]
    pub image: Option<Image>,
    #[prost(string, tag = "4")]
    pub app_id: String,
    #[prost(string, tag = "9")]
    pub builder_version: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImageGetOrCreateResponse {
    #[prost(string, tag = "1")]
    pub image_id: String,
    #[prost(message, optional, tag = "2")]
    pub result: Option<GenericResult>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImageJoinStreamingRequest {
    #[prost(string, tag = "1")]
    pub image_id: String,
    #[prost(float, tag = "2")]
    pub timeout: f32,
    #[prost(string, tag = "3")]
    pub last_entry_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskLogs {
    #[prost(string, tag = "1")]
    pub data: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImageJoinStreamingResponse {
    #[prost(message, optional, tag = "1")]
    pub result: Option<GenericResult>,
    #[prost(message, repeated, tag = "2")]
    pub task_logs: Vec<TaskLogs>,
    #[prost(string, tag = "3")]
    pub entry_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GpuConfig {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Resources {
    #[prost(uint32, tag = "2")]
    pub memory_mb: u32,
    #[prost(uint32, tag = "3")]
    pub milli_cpu: u32,
    #[prost(message, optional, tag = "4")]
    pub gpu_config: Option<GpuConfig>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PortSpec {
    #[prost(uint32, tag = "1")]
    pub port: u32,
    #[prost(bool, tag = "2")]
    pub unencrypted: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PortSpecs {
    #[prost(message, repeated, tag = "1")]
    pub ports: Vec<PortSpec>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct NetworkAccess {
    #[prost(int32, tag = "1")]
    pub network_access_type: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Sandbox {
    #[prost(string, repeated, tag = "1")]
    pub entrypoint_args: Vec<String>,
    #[prost(string, tag = "3")]
    pub image_id: String,
    #[prost(message, optional, tag = "5")]
    pub resources: Option<Resources>,
    #[prost(uint32, tag = "7")]
    pub timeout_secs: u32,
    #[prost(message, optional, tag = "20")]
    pub open_ports: Option<PortSpecs>,
    #[prost(message, optional, tag = "22")]
    pub network_access: Option<NetworkAccess>,
    #[prost(string, optional, tag = "30")]
    pub name: Option<String>,
    #[prost(uint32, optional, tag = "33")]
    pub idle_timeout_secs: Option<u32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct StringMap {
    #[prost(map = "string, string", tag = "1")]
    pub contents: HashMap<String, String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxTag {
    #[prost(string, tag = "1")]
    pub tag_name: String,
    #[prost(string, tag = "2")]
    pub tag_value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxCreateV2Request {
    #[prost(string, tag = "1")]
    pub app_id: String,
    #[prost(message, optional, tag = "2")]
    pub definition: Option<Sandbox>,
    #[prost(message, optional, tag = "3")]
    pub ephemeral_secrets: Option<StringMap>,
    #[prost(message, repeated, tag = "4")]
    pub tags: Vec<SandboxTag>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TunnelData {
    #[prost(string, tag = "1")]
    pub host: String,
    #[prost(uint32, tag = "2")]
    pub port: u32,
    #[prost(uint32, tag = "5")]
    pub container_port: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CommandRouterAccess {
    #[prost(string, tag = "1")]
    pub jwt: String,
    #[prost(string, tag = "2")]
    pub url: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxCreateV2Response {
    #[prost(string, tag = "1")]
    pub sandbox_id: String,
    #[prost(message, repeated, tag = "2")]
    pub tunnels: Vec<TunnelData>,
    #[prost(string, tag = "3")]
    pub task_id: String,
    #[prost(message, optional, tag = "5")]
    pub command_router_access: Option<CommandRouterAccess>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxId {
    #[prost(string, tag = "1")]
    pub sandbox_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxGetTunnelsRequest {
    #[prost(string, tag = "1")]
    pub sandbox_id: String,
    #[prost(float, tag = "2")]
    pub timeout: f32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxGetTunnelsResponse {
    #[prost(message, optional, tag = "1")]
    pub result: Option<GenericResult>,
    #[prost(message, repeated, tag = "2")]
    pub tunnels: Vec<TunnelData>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxGetTaskIdRequest {
    #[prost(string, tag = "1")]
    pub sandbox_id: String,
    #[prost(float, optional, tag = "2")]
    pub timeout: Option<f32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxGetTaskIdResponse {
    #[prost(string, optional, tag = "1")]
    pub task_id: Option<String>,
    #[prost(message, optional, tag = "2")]
    pub task_result: Option<GenericResult>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxGetCommandRouterAccessResponse {
    #[prost(string, tag = "1")]
    pub jwt: String,
    #[prost(string, tag = "2")]
    pub url: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxGetFromNameRequest {
    #[prost(string, tag = "1")]
    pub sandbox_name: String,
    #[prost(string, tag = "2")]
    pub environment_name: String,
    #[prost(string, tag = "3")]
    pub app_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxListRequest {
    #[prost(string, tag = "1")]
    pub app_id: String,
    #[prost(double, tag = "2")]
    pub before_timestamp: f64,
    #[prost(string, tag = "3")]
    pub environment_name: String,
    #[prost(bool, tag = "4")]
    pub include_finished: bool,
    #[prost(message, repeated, tag = "5")]
    pub tags: Vec<SandboxTag>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxInfo {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(double, tag = "3")]
    pub created_at: f64,
    #[prost(message, repeated, tag = "6")]
    pub tags: Vec<SandboxTag>,
    #[prost(string, tag = "7")]
    pub name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxListResponse {
    #[prost(message, repeated, tag = "1")]
    pub sandboxes: Vec<SandboxInfo>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SandboxTagsGetResponse {
    #[prost(message, repeated, tag = "1")]
    pub tags: Vec<SandboxTag>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskExecStartRequest {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub exec_id: String,
    #[prost(string, repeated, tag = "3")]
    pub command_args: Vec<String>,
    #[prost(int32, tag = "4")]
    pub stdout_config: i32,
    #[prost(int32, tag = "5")]
    pub stderr_config: i32,
    #[prost(uint32, optional, tag = "6")]
    pub timeout_secs: Option<u32>,
    #[prost(map = "string, string", tag = "12")]
    pub env: HashMap<String, String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskExec {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub exec_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskExecStdioReadRequest {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub exec_id: String,
    #[prost(uint64, tag = "3")]
    pub offset: u64,
    #[prost(int32, tag = "4")]
    pub file_descriptor: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskExecStdioReadResponse {
    #[prost(bytes = "vec", tag = "1")]
    pub data: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskExecWaitResponse {
    #[prost(int32, optional, tag = "1")]
    pub code: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub signal: Option<i32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskExecStdinWriteRequest {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub exec_id: String,
    #[prost(uint64, tag = "3")]
    pub offset: u64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
    #[prost(bool, tag = "5")]
    pub eof: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn test_an_exit_code_of_zero_is_still_an_exit_code() {
        let encoded = TaskExecWaitResponse {
            code: Some(0),
            signal: None,
        }
        .encode_to_vec();

        assert!(
            !encoded.is_empty(),
            "a oneof member is sent even at zero, which is how 0 differs from no answer"
        );
        assert_eq!(
            TaskExecWaitResponse::decode(encoded.as_slice())
                .expect("decoded")
                .code,
            Some(0)
        );
    }

    #[test]
    fn test_an_empty_gpu_config_is_still_sent() {
        let resources = Resources {
            memory_mb: 0,
            milli_cpu: 0,
            gpu_config: Some(GpuConfig {}),
        };

        assert_eq!(
            resources.encode_to_vec(),
            [0x22, 0x00],
            "the Go SDK always sends an empty GPU config, field 4"
        );
    }
}
