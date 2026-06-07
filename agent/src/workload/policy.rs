use super::types::{ActualState, DesiredState, WorkloadError, WorkloadKind, WorkloadResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RestartPolicy {
    Never,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkloadPolicy {
    initial_desired_state: DesiredState,
    restart_policy: RestartPolicy,
    start_rejection: Option<&'static str>,
    missing_runtime_task_error: &'static str,
}

impl WorkloadKind {
    pub(crate) fn policy(self) -> WorkloadPolicy {
        match self {
            Self::Once => WorkloadPolicy {
                initial_desired_state: DesiredState::Running,
                restart_policy: RestartPolicy::Never,
                start_rejection: Some("operation is only valid for service workloads"),
                missing_runtime_task_error: "runtime task disappeared before its exit was observed",
            },
            Self::Service => WorkloadPolicy {
                initial_desired_state: DesiredState::Running,
                restart_policy: RestartPolicy::Always,
                start_rejection: None,
                missing_runtime_task_error: "runtime task not found",
            },
        }
    }
}

impl WorkloadPolicy {
    pub(crate) fn initial_desired_state(self) -> DesiredState {
        self.initial_desired_state
    }

    pub(crate) fn ensure_start_allowed(self) -> WorkloadResult<()> {
        match self.start_rejection {
            Some(error) => Err(WorkloadError::failed_precondition(error)),
            None => Ok(()),
        }
    }

    pub(crate) fn should_restart_after_terminal(self, actual_state: ActualState) -> bool {
        matches!(actual_state, ActualState::Stopped | ActualState::Failed)
            && self.restart_policy == RestartPolicy::Always
    }

    pub(crate) fn missing_runtime_task_error(self) -> &'static str {
        self.missing_runtime_task_error
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::ErrorCode;
    use super::*;

    #[test]
    fn policy_defines_start_permissions() {
        let once = WorkloadKind::Once.policy();
        let service = WorkloadKind::Service.policy();

        let start_error = once.ensure_start_allowed().unwrap_err();
        assert_eq!(start_error.code(), ErrorCode::FailedPrecondition);
        assert_eq!(
            start_error.to_string(),
            "operation is only valid for service workloads"
        );

        service.ensure_start_allowed().unwrap();
    }

    #[test]
    fn policy_defines_restart_decisions() {
        assert!(
            !WorkloadKind::Once
                .policy()
                .should_restart_after_terminal(ActualState::Stopped)
        );
        assert!(
            !WorkloadKind::Once
                .policy()
                .should_restart_after_terminal(ActualState::Failed)
        );
        assert!(
            WorkloadKind::Service
                .policy()
                .should_restart_after_terminal(ActualState::Stopped)
        );
        assert!(
            WorkloadKind::Service
                .policy()
                .should_restart_after_terminal(ActualState::Failed)
        );
        assert!(
            !WorkloadKind::Service
                .policy()
                .should_restart_after_terminal(ActualState::Running)
        );
    }

    #[test]
    fn policy_defines_missing_task_errors() {
        assert_eq!(
            WorkloadKind::Once.policy().missing_runtime_task_error(),
            "runtime task disappeared before its exit was observed"
        );
        assert_eq!(
            WorkloadKind::Service.policy().missing_runtime_task_error(),
            "runtime task not found"
        );
    }
}
