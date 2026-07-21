#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownRequested;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequestReceived {
    pub method: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPromptSubmitted {
    pub workspace: String,
    pub prompt: String,
}
