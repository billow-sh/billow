use billow_api::api;
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkloadKind {
    Once,
    Service,
}

impl WorkloadKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Service => "service",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "once" => Some(Self::Once),
            "service" => Some(Self::Service),
            _ => None,
        }
    }

    pub(crate) fn from_proto(value: i32) -> Result<Self, WorkloadError> {
        match api::WorkloadKind::try_from(value) {
            Ok(api::WorkloadKind::Once) => Ok(Self::Once),
            Ok(api::WorkloadKind::Service) => Ok(Self::Service),
            _ => Err(WorkloadError::invalid_argument(
                "workload kind must be once or service",
            )),
        }
    }

    pub(crate) fn to_proto(self) -> api::WorkloadKind {
        match self {
            Self::Once => api::WorkloadKind::Once,
            Self::Service => api::WorkloadKind::Service,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DesiredState {
    Running,
    Stopped,
    Deleted,
}

impl DesiredState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Deleted => "deleted",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }

    pub(crate) fn to_proto(self) -> api::DesiredState {
        match self {
            Self::Running => api::DesiredState::Running,
            Self::Stopped => api::DesiredState::Stopped,
            Self::Deleted => api::DesiredState::Deleted,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActualState {
    Accepted,
    Creating,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
    Deleted,
}

impl ActualState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Creating => "creating",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Deleted => "deleted",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "accepted" => Some(Self::Accepted),
            "creating" => Some(Self::Creating),
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "stopping" => Some(Self::Stopping),
            "stopped" => Some(Self::Stopped),
            "failed" => Some(Self::Failed),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }

    pub(crate) fn to_proto(self) -> api::ActualState {
        match self {
            Self::Accepted => api::ActualState::Accepted,
            Self::Creating => api::ActualState::Creating,
            Self::Starting => api::ActualState::Starting,
            Self::Running => api::ActualState::Running,
            Self::Stopping => api::ActualState::Stopping,
            Self::Stopped => api::ActualState::Stopped,
            Self::Failed => api::ActualState::Failed,
            Self::Deleted => api::ActualState::Deleted,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Workload {
    pub(crate) id: String,
    pub(crate) kind: WorkloadKind,
    pub(crate) image: String,
    pub(crate) desired_state: DesiredState,
    pub(crate) actual_state: ActualState,
    pub(crate) runtime_task_id: Option<String>,
    pub(crate) container_ip: Option<String>,
    pub(crate) exit_code: Option<u32>,
    pub(crate) error: Option<String>,
    pub(crate) stopping_since_unix_secs: Option<i64>,
    pub(crate) runtime_unknown_since_unix_secs: Option<i64>,
    pub(crate) restart_attempts: i64,
    pub(crate) restart_not_before_unix_secs: Option<i64>,
    pub(crate) running_since_unix_secs: Option<i64>,
    pub(crate) created_at_unix_secs: i64,
    pub(crate) updated_at_unix_secs: i64,
}

impl Workload {
    pub(crate) fn to_proto(&self) -> api::WorkloadResponse {
        api::WorkloadResponse {
            workload_id: self.id.clone(),
            kind: self.kind.to_proto() as i32,
            image: self.image.clone(),
            desired_state: self.desired_state.to_proto() as i32,
            actual_state: self.actual_state.to_proto() as i32,
            runtime_task_id: self.runtime_task_id.clone().unwrap_or_default(),
            container_ip: self.container_ip.clone().unwrap_or_default(),
            exit_code: self.exit_code,
            error: self.error.clone().unwrap_or_default(),
            created_at_unix_secs: self.created_at_unix_secs,
            updated_at_unix_secs: self.updated_at_unix_secs,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CleanupState {
    Pending,
    Done,
    Failed,
}

impl CleanupState {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkloadRun {
    pub(crate) workload_id: String,
    pub(crate) runtime_task_id: String,
    pub(crate) container_ip: Option<String>,
    pub(crate) container_released: bool,
    pub(crate) release_attempts: i64,
    pub(crate) last_release_error: Option<String>,
    pub(crate) cleanup_state: CleanupState,
    pub(crate) cleanup_attempts: i64,
    pub(crate) last_cleanup_error: Option<String>,
    pub(crate) updated_at_unix_secs: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ErrorCode {
    InvalidArgument,
    NotFound,
    FailedPrecondition,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkloadError {
    code: ErrorCode,
    message: String,
}

impl WorkloadError {
    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub(crate) fn failed_precondition(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::FailedPrecondition, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub(crate) fn code(&self) -> ErrorCode {
        self.code
    }

    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for WorkloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for WorkloadError {}

pub(crate) type WorkloadResult<T> = Result<T, WorkloadError>;

pub(crate) fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

pub(crate) fn duration_secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

pub(crate) fn env_path_or_default(env_name: &str, default: &str) -> PathBuf {
    env::var_os(env_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

pub(crate) fn env_string_or_default(env_name: &str, default: &str) -> String {
    env::var(env_name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub(crate) fn env_duration_or_default(env_name: &str, default_secs: u64) -> Duration {
    let secs = env::var(env_name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(secs)
}
