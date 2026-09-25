use crate::env::Env;
use crate::init::ServiceDef;
use crate::runner::{RunError, Runner};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("{what}: {message}")]
    Nix { what: String, message: String },
    #[error(transparent)]
    Run(RunError),
    #[error("{what}: unexpected evaluator output")]
    Shape { what: String, source: serde_json::Error },
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

// one file a module renders; destinations are resolved later from name and key
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderedFile {
    pub name: String,
    pub key: String,
    pub content: String,
    pub executable: bool,
    pub scope: String,
    // set by lib.service; the init backend renders the actual files
    #[serde(default)]
    pub service: Option<ServiceDef>,
}

// a cached evaluation, valid while its key matches
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    key: String,
    value: Value,
}

// the last "error:" block of nix's output, dropping the trace above it
fn last_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().collect();
    let start = lines.iter().rposition(|line| line.trim_start().starts_with("error:")).unwrap_or(0);

    // dedent by the error line's indent so the snippet keeps its shape
    let indent = lines.get(start).map_or(0, |line| line.len() - line.trim_start().len());
    let block: Vec<&str> = lines[start..].iter().map(|line| line.get(indent..).unwrap_or(line.trim_start())).collect();
    block.join("\n").trim_start_matches("error: ").trim_end().to_string()
}

// a failed nix run becomes an error naming what was evaluated
fn nix_error(what: &str, error: RunError) -> EvalError {
    match error {
        RunError::Failed { stderr, .. } => EvalError::Nix { what: what.into(), message: last_error(&stderr) },
        other => EvalError::Run(other),
    }
}

// json of an evaluation, from the cache when the key matches, else from nix-instantiate; true if fresh
fn eval_cached(runner: &dyn Runner, what: &str, cache_file: &Path, key: &str, args: Vec<String>) -> Result<(Value, bool), EvalError> {
    let cached = fs::read(cache_file).ok().and_then(|bytes| serde_json::from_slice::<CacheEntry>(&bytes).ok());
    if let Some(entry) = cached.filter(|entry| entry.key == key) {
        return Ok((entry.value, false));
    }

    let stdout = runner.run("nix-instantiate", &args).map_err(|error| nix_error(what, error))?;
    let value: Value = serde_json::from_str(&stdout).map_err(|source| EvalError::Shape { what: what.into(), source })?;

    let io = |source| EvalError::Io { path: cache_file.into(), source };
    fs::create_dir_all(cache_file.parent().unwrap()).map_err(io)?;
    let entry = CacheEntry { key: key.into(), value };
    fs::write(cache_file, serde_json::to_vec(&entry).unwrap()).map_err(io)?;
    Ok((entry.value, true))
}

// nix-instantiate args shared by every evaluation, followed by the extra ones
fn nix_args(env: &Env, extra: &[String]) -> Vec<String> {
    let base = ["--eval", "--strict", "--json", "-I"].map(String::from);
    let maw = format!("maw={}", env.nix_dir().display());
    base.into_iter().chain([maw]).chain(extra.iter().cloned()).collect()
}

// evaluates a plain nix file such as registry.nix or maw.nix
pub fn eval_file(runner: &dyn Runner, env: &Env, file: &Path, cache_name: &str, key: &str) -> Result<Value, EvalError> {
    let args = nix_args(env, &[file.display().to_string()]);
    let what = file.display().to_string();
    Ok(eval_cached(runner, &what, &env.cache_dir().join(format!("{cache_name}.json")), key, args)?.0)
}

// evaluates the settings attrset (config.nix's maw), cached like a module
pub fn eval_settings(runner: &dyn Runner, env: &Env, repo_root: &Path, key: &str) -> Result<Value, EvalError> {
    let args = nix_args(env, &["-A".into(), "settings".into(), repo_root.display().to_string()]);
    Ok(eval_cached(runner, "config.nix maw settings", &env.cache_dir().join("settings.json"), key, args)?.0)
}

// one nixpkgs package's metadata through nix/nixpkgs-meta.nix, cached per package and nixpkgs revision
pub fn eval_nixpkgs_meta(runner: &dyn Runner, env: &Env, nixpkgs: &Path, attr: &str, revision: &str) -> Result<Value, EvalError> {
    let expression = env.nix_dir().join("nixpkgs-meta.nix").display().to_string();
    let args = nix_args(env, &[expression, "--arg".into(), "nixpkgs".into(), nixpkgs.display().to_string(), "--argstr".into(), "attr".into(), attr.into()]);
    let cache_file = env.cache_dir().join("nixpkgs").join(format!("{attr}.json"));
    Ok(eval_cached(runner, &format!("nixpkgs {attr}"), &cache_file, revision, args)?.0)
}

// evaluates modules.<name> of the dotfiles repo; true if nix actually ran
pub fn eval_module(runner: &dyn Runner, env: &Env, repo_root: &Path, name: &str, key: &str) -> Result<(Vec<RenderedFile>, bool), EvalError> {
    let args = nix_args(env, &["-A".into(), format!("modules.{name}"), repo_root.display().to_string()]);
    let cache_file = env.cache_dir().join("modules").join(format!("{name}.json"));
    let (value, fresh) = eval_cached(runner, &format!("modules/{name}.nix"), &cache_file, key, args)?;
    let files = serde_json::from_value(value).map_err(|source| EvalError::Shape { what: format!("modules.{name}"), source })?;
    Ok((files, fresh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    const FOOT: &str = r#"[{"name":"foot","key":"main","content":"[main]\n","executable":false,"scope":"user"}]"#;

    fn setup() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), &dir.path().join("share"));
        (dir, env)
    }

    #[test]
    fn module_eval_is_cached_by_key() {
        let (_dir, env) = setup();
        let runner = FakeRunner::new(|_, _| FOOT.into());

        let (files, fresh) = eval_module(&runner, &env, Path::new("/dots"), "foot", "k1").unwrap();
        assert!(fresh);
        assert_eq!(files[0].content, "[main]\n");

        // same key reuses the cache, a new key runs nix again
        assert!(!eval_module(&runner, &env, Path::new("/dots"), "foot", "k1").unwrap().1);
        assert!(eval_module(&runner, &env, Path::new("/dots"), "foot", "k2").unwrap().1);
        assert_eq!(runner.calls.borrow().len(), 2);
    }

    #[test]
    fn module_eval_passes_attr_and_repo() {
        let (_dir, env) = setup();
        let runner = FakeRunner::new(|_, _| FOOT.into());
        eval_module(&runner, &env, Path::new("/dots"), "foot", "k").unwrap();

        let call = &runner.calls.borrow()[0];
        assert!(call.starts_with("nix-instantiate --eval --strict --json -I maw="));
        assert!(call.ends_with("-A modules.foot /dots"));
    }

    #[test]
    fn last_error_drops_the_trace() {
        let stderr = "error:\n       … while evaluating\n         at lists.nix:448\n\n       error: undefined variable 'x'\n       at m.nix:1:5:\n            1| x\n";
        assert_eq!(last_error(stderr), "undefined variable 'x'\nat m.nix:1:5:\n     1| x");
    }

    #[test]
    fn bad_output_is_a_shape_error() {
        let (_dir, env) = setup();
        let runner = FakeRunner::new(|_, _| "{}".into());
        let result = eval_module(&runner, &env, Path::new("/dots"), "foot", "k");
        assert!(matches!(result, Err(EvalError::Shape { .. })));
    }
}
