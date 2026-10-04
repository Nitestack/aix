#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchContext {
    pub profile: String,
    pub logical_tool_name: String,
    pub run_id: Option<String>,
    pub run_policy: Option<String>,
}

impl LaunchContext {
    pub(crate) fn new(
        profile: String,
        logical_tool_name: String,
        run_id: Option<String>,
        run_policy: Option<String>,
    ) -> Self {
        Self {
            profile,
            logical_tool_name,
            run_id,
            run_policy,
        }
    }
}
