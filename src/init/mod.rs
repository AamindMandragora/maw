use crate::env::Env;
use crate::runner::{RunError, Runner};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub mod runit;

// who runs a service: the system at boot, or the user's session
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    System,
    User,
}

impl Scope {
    // "user" is user scope; anything else a module can say ("system", "root") is system
    pub fn parse(text: &str) -> Scope {
        if text == "user" { Scope::User } else { Scope::System }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.pad(if *self == Scope::User { "user" } else { "system" })
    }
}

fn yes() -> bool {
    true
}

// what lib.service describes; the init backend decides what files that becomes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceDef {
    pub run: String,
    #[serde(default = "yes")]
    pub log: bool,
    #[serde(default = "yes")]
    pub enable: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

// a service supervisor: how services are written to disk, enabled, and controlled
pub trait InitBackend {
    // a service's files relative to its definition dir, as (path, content, executable)
    fn render(&self, name: &str, scope: Scope, service: &ServiceDef) -> Vec<(String, String, bool)>;

    // where a service's definition lives
    fn definition(&self, env: &Env, scope: Scope, name: &str) -> PathBuf;

    // the link whose presence means the service is enabled
    fn enabled_link(&self, env: &Env, scope: Scope, name: &str) -> PathBuf;

    // the file its log is written to
    fn log_file(&self, env: &Env, scope: Scope, name: &str) -> PathBuf;

    // runs an action like up, down, restart, or status on the terminal, as root for system services
    fn control(&self, env: &Env, runner: &dyn Runner, scope: Scope, name: &str, action: &str) -> Result<(), RunError>;

    // a one-word state like run or down, when it can be read without a password
    fn state(&self, env: &Env, runner: &dyn Runner, scope: Scope, name: &str) -> Option<String>;
}
