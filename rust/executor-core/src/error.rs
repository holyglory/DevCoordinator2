use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutorError {
    message: String,
}

impl ExecutorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExecutorError {}

impl From<std::io::Error> for ExecutorError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<devcoordinator2_executor_protocol::ContractError> for ExecutorError {
    fn from(error: devcoordinator2_executor_protocol::ContractError) -> Self {
        Self::new(error.to_string())
    }
}
