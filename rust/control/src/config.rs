use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub socket_path: PathBuf,
}

impl Config {
    pub fn load() -> Self {
        Self {
            socket_path: std::env::var_os("DEVCOORDINATOR2_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/run/devcoordinator2/daemon.sock")),
        }
    }
}
