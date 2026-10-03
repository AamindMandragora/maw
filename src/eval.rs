use crate::env::Env;
use crate::init::ServiceDef;
use crate::runner::{RunError, Runner};
use std::collections::BTreeMap;
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
    // lib.program's reload, a command making the running program read changed config
    #[serde(default)]
    pub reload: Option<String>,
    // set by lib.dconf: GVariant text by full key path
    #[serde(default)]
    pub dconf: Option<BTreeMap<String, String>>,
    // the same file rendered with placeholder colors, when there's a theme
    #[serde(default)]
    pub plain: Option<Box<RenderedFile>>,
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

// a cached value, if its key still matches
fn cached(cache_file: &Path, key: &str) -> Option<Value> {
    let entry = serde_json::from_slice::<CacheEntry>(&fs::read(cache_file).ok()?).ok()?;
    (entry.key == key).then_some(entry.value)
}

fn save_cache(cache_file: &Path, key: &str, value: &Value) -> Result<(), EvalError> {
    let io = |source| EvalError::Io { path: cache_file.into(), source };
    fs::create_dir_all(cache_file.parent().unwrap()).map_err(io)?;
    fs::write(cache_file, serde_json::to_vec(&CacheEntry { key: key.into(), value: value.clone() }).unwrap()).map_err(io)
}

// json of an evaluation, from the cache when the key matches, else from nix-instantiate; true if fresh
fn eval_cached(runner: &dyn Runner, what: &str, cache_file: &Path, key: &str, args: Vec<String>) -> Result<(Value, bool), EvalError> {
    if let Some(value) = cached(cache_file, key) {
        return Ok((value, false));
    }
    let stdout = runner.run("nix-instantiate", &args).map_err(|error| nix_error(what, error))?;
    let value: Value = serde_json::from_str(&stdout).map_err(|source| EvalError::Shape { what: what.into(), source })?;
    save_cache(cache_file, key, &value)?;
    Ok((value, true))
}

// nix-instantiate args shared by every evaluation, followed by the extra ones
fn nix_args(env: &Env, extra: &[String]) -> Vec<String> {
    let base = ["--eval", "--strict", "--json", "-I"].map(String::from);
    let maw = format!("maw={}", env.nix_dir().display());
    let state = format!("maw-state={}", env.state_dir.display());
    let config = format!("maw-config={}", env.config_dir.display());
    base.into_iter().chain([maw, "-I".into(), state, "-I".into(), config]).chain(extra.iter().cloned()).collect()
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
    let (value, fresh) = eval_cached(runner, &format!("modules/{name}.nix"), &module_cache(env, name), key, args)?;
    Ok((files(name, value)?, fresh))
}

fn module_cache(env: &Env, name: &str) -> PathBuf {
    env.cache_dir().join("modules").join(format!("{name}.json"))
}

fn files(name: &str, value: Value) -> Result<Vec<RenderedFile>, EvalError> {
    serde_json::from_value(value).map_err(|source| EvalError::Shape { what: format!("modules.{name}"), source })
}

// modules as (name, cache key), each with its files and whether it was evaluated now. Every module whose cache is
// stale is evaluated in one nix-instantiate, so nix starts and reads maw's lib once; if that fails, they're evaluated
// one at a time, so the error names the module it came from
pub fn eval_modules(runner: &dyn Runner, env: &Env, repo_root: &Path, modules: &[(String, String)]) -> Result<Vec<(String, Vec<RenderedFile>, bool)>, EvalError> {
    let stale: Vec<&(String, String)> = modules.iter().filter(|(name, key)| cached(&module_cache(env, name), key).is_none()).collect();
    let plain_names = stale.iter().all(|(name, _)| name.chars().all(|char| char.is_ascii_alphanumeric() || "._+-".contains(char)));
    if stale.len() > 1 && plain_names {
        let names: String = stale.iter().map(|(name, _)| format!(" \"{name}\"")).collect();
        let expression = format!("{{ root }}: let modules = (import (/. + root)).modules; in {{ inherit (modules){names}; }}");
        let args = nix_args(env, &["-E".into(), expression, "--argstr".into(), "root".into(), repo_root.display().to_string()]);
        let batch = runner.run("nix-instantiate", &args).ok().and_then(|stdout| serde_json::from_str::<BTreeMap<String, Value>>(&stdout).ok());
        // each module's result cached as if evaluated alone; a failed batch leaves the caches for one at a time
        batch.iter().flatten().try_for_each(|(name, value)| match stale.iter().find(|(stale, _)| stale == name) {
            Some((_, key)) => save_cache(&module_cache(env, name), key, value),
            None => Ok(()),
        })?;
    }
    modules
        .iter()
        .map(|(name, key)| {
            let (files, _) = eval_module(runner, env, repo_root, name, key)?;
            Ok((name.clone(), files, stale.iter().any(|(stale, _)| stale == name)))
        })
        .collect()
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
    fn stale_modules_are_evaluated_together() {
        let (_dir, env) = setup();
        let batch = format!(r#"{{"foot":{FOOT},"niri":{FOOT}}}"#);
        let runner = FakeRunner::new(move |_, args| if args.contains(&"-E".to_string()) { batch.clone() } else { FOOT.into() });
        let modules = [("foot".to_string(), "k1".to_string()), ("niri".to_string(), "k1".to_string())];

        let first = eval_modules(&runner, &env, Path::new("/dots"), &modules).unwrap();
        assert_eq!(first.iter().map(|(name, files, fresh)| (name.as_str(), files.len(), *fresh)).collect::<Vec<_>>(), [("foot", 1, true), ("niri", 1, true)]);
        assert_eq!(runner.calls.borrow().len(), 1);

        // both cached now; a changed key alone is evaluated alone
        eval_modules(&runner, &env, Path::new("/dots"), &[modules[0].clone(), ("niri".into(), "k2".into())]).unwrap();
        assert_eq!(runner.calls.borrow().len(), 2);
        assert!(runner.calls.borrow()[1].contains("-A modules.niri"));
    }

    #[test]
    fn a_failed_batch_finds_the_module_to_blame() {
        let (_dir, env) = setup();
        let runner = FakeRunner::fallible(|_, args| match args.iter().find(|arg| arg.starts_with("modules.")).map(String::as_str) {
            Some("modules.bad") => Err(RunError::Failed { program: "nix-instantiate".into(), stderr: "error: undefined variable 'x'".into() }),
            Some(_) => Ok(FOOT.into()),
            None => Err(RunError::Failed { program: "nix-instantiate".into(), stderr: "error: undefined variable 'x'".into() }),
        });
        let modules = [("foot".to_string(), "k".to_string()), ("bad".to_string(), "k".to_string())];
        let error = eval_modules(&runner, &env, Path::new("/dots"), &modules).unwrap_err();
        assert_eq!(error.to_string(), "modules/bad.nix: undefined variable 'x'");
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
