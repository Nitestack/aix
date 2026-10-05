use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchContext {
    pub profile: String,
    pub logical_tool_name: String,
    pub run_id: Option<String>,
    pub run_policy: Option<String>,
    pub enforcement: RequestEnforcement,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RequestEnforcement {
    pub allowed_models: Vec<String>,
    pub deadline: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicyRejection {
    DeadlineExceeded,
    MissingModel,
    ModelNotAllowed,
}

impl RequestEnforcement {
    pub(crate) fn check_deadline(&self, now: Instant) -> Result<(), PolicyRejection> {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            Err(PolicyRejection::DeadlineExceeded)
        } else {
            Ok(())
        }
    }

    pub(crate) fn check_model(&self, model: Option<&str>) -> Result<(), PolicyRejection> {
        if self.allowed_models.is_empty() {
            return Ok(());
        }
        match model {
            None => Err(PolicyRejection::MissingModel),
            Some(model) if self.allowed_models.iter().any(|allowed| allowed == model) => Ok(()),
            Some(_) => Err(PolicyRejection::ModelNotAllowed),
        }
    }
}

impl LaunchContext {
    #[cfg(test)]
    pub(crate) fn new(
        profile: String,
        logical_tool_name: String,
        run_id: Option<String>,
        run_policy: Option<String>,
    ) -> Self {
        Self::with_enforcement(
            profile,
            logical_tool_name,
            run_id,
            run_policy,
            RequestEnforcement::default(),
        )
    }

    pub(crate) fn with_enforcement(
        profile: String,
        logical_tool_name: String,
        run_id: Option<String>,
        run_policy: Option<String>,
        enforcement: RequestEnforcement,
    ) -> Self {
        Self {
            profile,
            logical_tool_name,
            run_id,
            run_policy,
            enforcement,
        }
    }
}
