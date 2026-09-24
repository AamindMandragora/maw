use super::{Meta, ScaffoldError, SourcePkg};
use crate::env::Env;
use crate::eval;
use crate::runner::Runner;
use std::fs;
use std::path::{Path, PathBuf};

const NIXPKGS: &str = "https://github.com/NixOS/nixpkgs";
const BRANCH: &str = "nixos-unstable";

// maw's shallow nixos-unstable clone, only ever evaluated, never built from
pub struct Nixpkgs<'a> {
    runner: &'a dyn Runner,
    env: Env,
    clone: PathBuf,
}

impl<'a> Nixpkgs<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env, clone: &Path) -> Self {
        Nixpkgs { runner, env: env.clone(), clone: clone.into() }
    }

    fn git(&self, args: &[&str]) -> Vec<String> {
        ["-C".to_string(), self.clone.display().to_string()].into_iter().chain(args.iter().map(|arg| arg.to_string())).collect()
    }

    // clones on first use
    fn ensure(&self) -> Result<(), ScaffoldError> {
        if self.clone.join("default.nix").exists() {
            return Ok(());
        }
        fs::create_dir_all(self.clone.parent().unwrap()).map_err(|source| ScaffoldError::Io { path: self.clone.clone(), source })?;
        let args = ["clone", "--depth", "1", "--branch", BRANCH, NIXPKGS].map(String::from).into_iter().chain([self.clone.display().to_string()]).collect::<Vec<_>>();
        Ok(self.runner.interactive("git", &args)?)
    }

    // the checked-out commit, which keys the metadata cache
    pub fn revision(&self) -> Result<String, ScaffoldError> {
        self.ensure()?;
        Ok(self.runner.run("git", &self.git(&["rev-parse", "--short=12", "HEAD"]))?.trim().to_string())
    }

    // moves the clone to the branch's latest commit, still shallow
    pub fn pull(&self) -> Result<(), ScaffoldError> {
        self.ensure()?;
        self.runner.interactive("git", &self.git(&["fetch", "--depth", "1", "origin", BRANCH]))?;
        self.runner.run("git", &self.git(&["reset", "-q", "--hard", "FETCH_HEAD"]))?;
        Ok(())
    }

    // one package's metadata, evaluated once per nixpkgs revision
    pub fn meta(&self, attr: &str) -> Result<(Meta, String), ScaffoldError> {
        let revision = self.revision()?;
        let value = eval::eval_nixpkgs_meta(self.runner, &self.env, &self.clone, attr, &revision)?;
        let meta = serde_json::from_value(value).map_err(|source| ScaffoldError::Meta { attr: attr.into(), source })?;
        Ok((meta, revision))
    }

    // a source package named name, from nixpkgs' attr
    pub fn source_pkg(&self, name: &str, attr: &str) -> Result<SourcePkg, ScaffoldError> {
        let (meta, revision) = self.meta(attr)?;
        SourcePkg::from_meta(&meta, name, &format!("nixpkgs '{attr}' at {revision}"))
    }
}
