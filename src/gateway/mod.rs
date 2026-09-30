mod litellm;
#[allow(dead_code)] // Prepared for upcoming OpenAI-compatible commands.
pub(crate) mod openai;
mod transport;

pub(crate) use litellm::LiteLlmAdminClient;
