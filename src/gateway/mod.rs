mod litellm;
#[allow(dead_code)] // Chat completions is intended for a future inference command.
pub(crate) mod openai;
mod transport;

pub(crate) use litellm::LiteLlmAdminClient;
pub(crate) use openai::OpenAiClient;
