use crate::env::Env;
use crate::runner::{RunError, Runner};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("no dotfiles repo; run `maw init <path>`")]
    NotInitialized,
    #[error("dotfiles repo {0} is missing")]
    Missing(PathBuf),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error(transparent)]
    Run(#[from] RunError),
}

const DEFAULT_NIX: &str = "import <maw> { dotfiles = ./.; }\n";

const CONFIG_NIX: &str = "{
  # values shared between modules: fonts, colors, paths
}
";

const MAW_NIX: &str = "{
  packages = { };
  services = { };
  paths = { };
}
";

// the user's dotfiles repo
#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> RepoError + '_ {
    move |source| RepoError::Io { path: path.into(), source }
}

// runs make unless the path exists; the path if it was created
fn create_missing(path: PathBuf, make: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<Option<PathBuf>, RepoError> {
    if path.exists() {
        return Ok(None);
    }
    make(&path).map_err(io(&path))?;
    Ok(Some(path))
}

// file holding the repo path, and nothing else
fn pointer(env: &Env) -> PathBuf {
    env.config_dir.join("dotfiles")
}

impl Repo {
    // the repo recorded by `maw init`
    pub fn locate(env: &Env) -> Result<Repo, RepoError> {
        let recorded = fs::read_to_string(pointer(env)).map_err(|_| RepoError::NotInitialized)?;
        let root = PathBuf::from(recorded.trim());
        if !root.is_dir() {
            return Err(RepoError::Missing(root));
        }
        Ok(Repo { root })
    }

    // scaffolds missing files, git-inits if needed, and records the repo; returns what it created
    pub fn init(env: &Env, runner: &dyn Runner, path: &Path) -> Result<(Repo, Vec<PathBuf>), RepoError> {
        fs::create_dir_all(path).map_err(io(path))?;
        let root = fs::canonicalize(path).map_err(io(path))?;
        let repo = Repo { root };

        let files = [("default.nix", DEFAULT_NIX), ("config.nix", CONFIG_NIX), ("maw.nix", MAW_NIX)];
        let dirs = ["modules", "static", "out"];

        // only what doesn't exist yet, so init is safe on an existing repo
        let made_files = files.iter().map(|(name, content)| create_missing(repo.root.join(name), |file| fs::write(file, content)));
        let made_dirs = dirs.iter().map(|name| create_missing(repo.root.join(name), |dir| fs::create_dir(dir)));
        let mut created: Vec<PathBuf> = made_files.chain(made_dirs).filter_map(Result::transpose).collect::<Result<_, _>>()?;

        if !repo.root.join(".git").exists() {
            runner.run("git", &["-C".into(), repo.root.display().to_string(), "init".into(), "-q".into()])?;
            created.push(repo.root.join(".git"));
        }

        fs::create_dir_all(&env.config_dir).map_err(io(&env.config_dir))?;
        fs::write(pointer(env), format!("{}\n", repo.root.display())).map_err(io(&pointer(env)))?;
        Ok((repo, created))
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.nix")
    }

    pub fn maw_file(&self) -> PathBuf {
        self.root.join("maw.nix")
    }

    pub fn out_dir(&self) -> PathBuf {
        self.root.join("out")
    }

    pub fn srcpkgs_dir(&self) -> PathBuf {
        self.root.join("srcpkgs")
    }

    pub fn static_dir(&self) -> PathBuf {
        self.root.join("static")
    }

    pub fn module_file(&self, name: &str) -> PathBuf {
        self.root.join("modules").join(format!("{name}.nix"))
    }

    // names of every modules/<name>.nix, sorted
    pub fn module_names(&self) -> Result<Vec<String>, RepoError> {
        let dir = self.root.join("modules");
        let entries = fs::read_dir(&dir).map_err(io(&dir))?;

        // stems of regular .nix files only
        let mut names: Vec<String> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "nix"))
            .filter_map(|path| Some(path.file_stem()?.to_string_lossy().into_owned()))
            .collect();
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    fn setup() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), &dir.path().join("share"));
        (dir, env)
    }

    #[test]
    fn init_scaffolds_and_records_repo() {
        let (dir, env) = setup();
        let runner = FakeRunner::new(|_, _| String::new());
        let (repo, created) = Repo::init(&env, &runner, &dir.path().join("dots")).unwrap();

        assert!(repo.maw_file().is_file() && repo.out_dir().is_dir());
        assert_eq!(created.len(), 7);
        assert_eq!(runner.calls.borrow()[0], format!("git -C {} init -q", repo.root.display()));
        assert_eq!(Repo::locate(&env).unwrap().root, repo.root);
    }

    #[test]
    fn init_keeps_existing_files() {
        let (dir, env) = setup();
        let runner = FakeRunner::new(|_, _| String::new());
        let path = dir.path().join("dots");
        fs::create_dir_all(path.join(".git")).unwrap();
        fs::write(path.join("config.nix"), "{ mine = 1; }").unwrap();

        let (repo, created) = Repo::init(&env, &runner, &path).unwrap();
        assert_eq!(fs::read_to_string(repo.config_file()).unwrap(), "{ mine = 1; }");
        assert!(!created.contains(&repo.config_file()));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn locate_without_init_fails() {
        let (_dir, env) = setup();
        assert!(matches!(Repo::locate(&env), Err(RepoError::NotInitialized)));
    }

    #[test]
    fn module_names_are_sorted_nix_stems() {
        let (dir, env) = setup();
        let (repo, _) = Repo::init(&env, &FakeRunner::new(|_, _| String::new()), &dir.path().join("dots")).unwrap();
        ["waybar.nix", "fuzzel.nix", "notes.txt"].iter().for_each(|file| fs::write(repo.root.join("modules").join(file), "").unwrap());
        assert_eq!(repo.module_names().unwrap(), ["fuzzel", "waybar"]);
    }
}
