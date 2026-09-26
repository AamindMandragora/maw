use crate::env::Env;
use crate::backup;
use crate::inputs::{Inputs, InputsError, hash_bytes, save_json};
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// encrypted files: static/<name>/<file>.age, encrypted with age to every machine's ssh key in hosts/<name>.pub and
// decrypted with this machine's own into the state dir, never the repo; the live file links to the decrypted copy

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("age isn't installed; `maw install age` first")]
    NoAge,
    #[error("no ssh key at {0}; make one with `ssh-keygen -t ed25519`, or set maw.secretKey in config.nix")]
    NoKey(String),
    #[error("{0} isn't encrypted for this machine yet; `maw secret rekey` on one it is, once hosts/{1}.pub is pushed")]
    NotRecipient(String, String),
    #[error("{0} isn't an encrypted file maw manages")]
    Unknown(String),
    #[error("not decrypted yet; `maw activate` decrypts it")]
    Pending,
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Inputs(#[from] InputsError),
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> SecretsError + '_ {
    move |source| SecretsError::Io { path: path.into(), source }
}

// this machine's private ssh key: maw.secretKey, else ~/.ssh/id_ed25519, else ~/.ssh/id_rsa
pub fn key(env: &Env, configured: Option<&str>) -> Result<PathBuf, SecretsError> {
    let in_home = |path: &str| env.home.join(path.strip_prefix("~/").unwrap_or(path));
    let candidates: Vec<PathBuf> = match configured {
        Some(path) => vec![in_home(path)],
        None => vec![in_home(".ssh/id_ed25519"), in_home(".ssh/id_rsa")],
    };
    candidates.iter().find(|key| key.is_file() && key.with_extension("pub").is_file()).cloned().ok_or_else(|| SecretsError::NoKey(env.pretty(&candidates[0])))
}

fn hosts_dir(repo: &Repo) -> PathBuf {
    repo.root.join("hosts")
}

// every machine's public key, from hosts/<name>.pub
pub fn recipients(repo: &Repo) -> Vec<PathBuf> {
    let files = fs::read_dir(hosts_dir(repo)).into_iter().flatten().filter_map(|entry| entry.ok()).map(|entry| entry.path());
    let mut keys: Vec<PathBuf> = files.filter(|path| path.extension().is_some_and(|ext| ext == "pub")).collect();
    keys.sort();
    keys
}

// publishes this machine's public key as hosts/<name>.pub the first time; the file when it's new
pub fn ensure_recipient(repo: &Repo, key: &Path) -> Result<Option<PathBuf>, SecretsError> {
    let file = hosts_dir(repo).join(format!("{}.pub", repo.host));
    if file.exists() {
        return Ok(None);
    }
    let public = key.with_extension("pub");
    fs::create_dir_all(hosts_dir(repo)).map_err(io(&file))?;
    fs::copy(&public, &file).map_err(io(&public))?;
    Ok(Some(file))
}

// age with its messages captured; it asks for a key's passphrase on the terminal device itself, so prompts still show
fn age(runner: &dyn Runner, args: Vec<String>) -> Result<(), SecretsError> {
    runner.run("age", &args).map(drop).map_err(|error| match error {
        RunError::Spawn { .. } => SecretsError::NoAge,
        error => error.into(),
    })
}

// why an encrypted file can't be placed, without its name, and what to do when it isn't for this machine yet
pub fn reason(repo: &Repo, error: &SecretsError) -> String {
    let published = hosts_dir(repo).join(format!("{}.pub", repo.host)).exists();
    match error {
        SecretsError::NotRecipient(..) if published => "not encrypted for this machine yet; `maw secret rekey` on one it is, then pull".into(),
        SecretsError::NotRecipient(..) => format!("not encrypted for this machine; `maw secret rekey` here records hosts/{}.pub, then push and rekey on one it is", repo.host),
        error => error.to_string(),
    }
}

// encrypts a file to every machine's key
pub fn encrypt(runner: &dyn Runner, repo: &Repo, plain: &Path, encrypted: &Path) -> Result<(), SecretsError> {
    fs::create_dir_all(encrypted.parent().unwrap()).map_err(io(encrypted))?;
    let to = recipients(repo).into_iter().flat_map(|recipient| ["-R".to_string(), recipient.display().to_string()]);
    let args = to.chain(["-o".to_string(), encrypted.display().to_string(), plain.display().to_string()]).collect();
    age(runner, args)
}

// decrypts with this machine's key into a file only the user can read; one this machine can't open is NotRecipient
pub fn decrypt(runner: &dyn Runner, repo: &Repo, key: &Path, encrypted: &Path, plain: &Path) -> Result<(), SecretsError> {
    let dir = plain.parent().unwrap();
    fs::create_dir_all(dir).map_err(io(dir))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io(dir))?;
    let args = ["-d", "-i", &key.display().to_string(), "-o", &plain.display().to_string(), &encrypted.display().to_string()].map(String::from).to_vec();
    // only a file with no stanza for this key is foreign; a wrong passphrase or ^C is an ordinary failure
    match age(runner, args) {
        Err(SecretsError::Run(RunError::Failed { stderr, .. })) if stderr.contains("no identity matched") => {
            Err(SecretsError::NotRecipient(encrypted.strip_prefix(&repo.root).unwrap_or(encrypted).display().to_string(), repo.host.clone()))
        }
        result => result,
    }?;
    fs::set_permissions(plain, fs::Permissions::from_mode(0o600)).map_err(io(plain))
}

// every encrypted file under static/
pub fn encrypted_files(repo: &Repo) -> Vec<PathBuf> {
    let files = crate::activate::walk(&repo.static_dir()).unwrap_or_default();
    files.into_iter().filter(|path| path.extension().is_some_and(|ext| ext == "age")).collect()
}

// where an encrypted file's plaintext lives: under the state dir, at its static/ path without .age
pub fn plain_path(env: &Env, repo: &Repo, encrypted: &Path) -> PathBuf {
    let relative = encrypted.strip_prefix(repo.static_dir()).unwrap_or(encrypted);
    env.state_dir.join("secrets").join(relative).with_extension("")
}

// what maw knows of each encrypted file: the hashes of the encrypted file and its plaintext when it last decrypted it,
// and the hash of encrypted files that weren't for this machine
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Record {
    #[serde(default)]
    seen: BTreeMap<PathBuf, Seen>,
    #[serde(default)]
    foreign: BTreeMap<PathBuf, String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Seen {
    encrypted: String,
    plain: String,
}

fn record_file(env: &Env) -> PathBuf {
    env.state_dir.join("secrets.json")
}

// the record, or an empty one when there's none or it's in an older shape, which only costs a decrypt
fn load_record(env: &Env) -> Record {
    fs::read_to_string(record_file(env)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save_record(env: &Env, record: &Record) -> Result<(), SecretsError> {
    Ok(save_json(&record_file(env), record)?)
}

// a decrypted file in place, and where an edited copy it replaced went
#[derive(Debug, PartialEq)]
pub struct Placed {
    pub plain: PathBuf,
    pub backup: Option<PathBuf>,
}

// the plaintext of an encrypted file, decrypted only when the encrypted file changed since the last time, so a key with a
// passphrase is rarely asked; decrypt false (a dry run) never runs age. A file that wasn't for this machine isn't tried
// again until it changes, and a decrypted copy edited through its link is backed up before a new one replaces it
pub fn plaintext(env: &Env, runner: &dyn Runner, repo: &Repo, key: &Path, encrypted: &Path, inputs: &mut Inputs, decrypt_now: bool) -> Result<Placed, SecretsError> {
    let plain = plain_path(env, repo, encrypted);
    let mut record = load_record(env);
    let hash = inputs.hash(encrypted)?;
    if record.foreign.get(encrypted) == Some(&hash) {
        return Err(SecretsError::NotRecipient(encrypted.strip_prefix(&repo.root).unwrap_or(encrypted).display().to_string(), repo.host.clone()));
    }
    let seen = record.seen.get(encrypted);
    if seen.is_some_and(|seen| seen.encrypted == hash) && plain.is_file() {
        return Ok(Placed { plain, backup: None });
    }
    if !decrypt_now {
        return Err(SecretsError::Pending);
    }

    // decrypted beside the old copy first, so a failure leaves the old one in place
    let fresh = plain.with_file_name(format!(".{}.new", plain.file_name().unwrap_or_default().to_string_lossy()));
    match decrypt(runner, repo, key, encrypted, &fresh) {
        Err(SecretsError::NotRecipient(file, host)) => {
            record.foreign.insert(encrypted.into(), hash);
            save_record(env, &record)?;
            return Err(SecretsError::NotRecipient(file, host));
        }
        result => result?,
    }

    // an old copy that no longer matches what maw decrypted was edited through its link: kept aside, not lost
    let edited = plain.is_file() && seen.is_some_and(|seen| hash_file(&plain).ok().as_ref() != Some(&seen.plain));
    let backup = if edited { Some(backup::backup(env, &plain).map_err(io(&plain))?) } else { None };
    fs::rename(&fresh, &plain).map_err(io(&plain))?;

    record.seen.insert(encrypted.into(), Seen { encrypted: hash, plain: hash_file(&plain)? });
    record.foreign.remove(encrypted);
    save_record(env, &record)?;
    Ok(Placed { plain, backup })
}

// records plaintext maw already has for an encrypted file, so the next activation needn't decrypt it again
pub fn store(env: &Env, repo: &Repo, encrypted: &Path, plain_source: &Path, inputs: &mut Inputs) -> Result<(), SecretsError> {
    let plain = plain_path(env, repo, encrypted);
    let dir = plain.parent().unwrap();
    fs::create_dir_all(dir).map_err(io(dir))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io(dir))?;
    fs::copy(plain_source, &plain).map_err(io(plain_source))?;
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o600)).map_err(io(&plain))?;

    let mut record = load_record(env);
    record.seen.insert(encrypted.into(), Seen { encrypted: inputs.hash(encrypted)?, plain: hash_file(&plain)? });
    record.foreign.remove(encrypted);
    save_record(env, &record)
}

// a private scratch copy for editing or rekeying, removed by the caller
pub fn scratch(env: &Env, encrypted: &Path) -> Result<PathBuf, SecretsError> {
    let dir = env.state_dir.join("secret-edit");
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(io(&dir))?;
    Ok(dir.join(encrypted.file_stem().unwrap_or_default()))
}

// re-encrypts every encrypted file to every machine's key; returns what was rekeyed, and what this machine couldn't open
pub fn rekey(env: &Env, runner: &dyn Runner, repo: &Repo, key: &Path) -> Result<(Vec<PathBuf>, Vec<SecretsError>), SecretsError> {
    let mut inputs = Inputs::default();
    let results = encrypted_files(repo).into_iter().map(|encrypted| {
        let temp = scratch(env, &encrypted)?;
        let result = decrypt(runner, repo, key, &encrypted, &temp).and_then(|_| encrypt(runner, repo, &temp, &encrypted)).and_then(|_| store(env, repo, &encrypted, &temp, &mut inputs));
        fs::remove_file(&temp).ok();
        result.map(|_| encrypted)
    });
    let (done, failed): (Vec<_>, Vec<_>) = results.partition(Result::is_ok);
    let failed: Vec<SecretsError> = failed.into_iter().filter_map(Result::err).collect();
    if let Some(position) = failed.iter().position(|error| !matches!(error, SecretsError::NotRecipient(..))) {
        return Err(failed.into_iter().nth(position).unwrap());
    }
    Ok((done.into_iter().filter_map(Result::ok).collect(), failed))
}

// an encrypted file named by its static/ path or by the live file linked to its plaintext
pub fn find(env: &Env, repo: &Repo, given: &Path) -> Result<PathBuf, SecretsError> {
    let absolute = std::path::absolute(given).map_err(io(given))?;
    if absolute.extension().is_some_and(|ext| ext == "age") && absolute.is_file() {
        return Ok(absolute);
    }
    let target = fs::read_link(&absolute).unwrap_or(absolute.clone());
    let found = encrypted_files(repo).into_iter().find(|encrypted| plain_path(env, repo, encrypted) == target);
    found.ok_or_else(|| SecretsError::Unknown(env.pretty(&absolute)))
}

// the hash of a file's bytes, for placing a decrypted copy like any static file
pub fn hash_file(path: &Path) -> Result<String, SecretsError> {
    Ok(hash_bytes(&fs::read(path).map_err(io(path))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::testing::{fixture, write};

    // age that "decrypts" by copying the input to -o, failing for files named foreign.age, and as a wrong passphrase would
    // for ones named locked.age
    fn age() -> FakeRunner {
        FakeRunner::fallible(|_, args| {
            let output = args.iter().position(|arg| arg == "-o").map(|at| args[at + 1].clone()).unwrap();
            let input = args.last().unwrap();
            if input.ends_with("foreign.age") {
                return Err(RunError::Failed { program: "age".into(), stderr: "age: error: no identity matched any of the recipients".into() });
            }
            if input.ends_with("locked.age") {
                return Err(RunError::Failed { program: "age".into(), stderr: "age: error: incorrect passphrase".into() });
            }
            fs::copy(input, output).unwrap();
            Ok(String::new())
        })
    }

    #[test]
    fn keys_are_found_and_published_once() {
        let fixture = fixture(&[]);
        assert!(matches!(key(&fixture.env, None), Err(SecretsError::NoKey(_))));
        write(&fixture.env.home.join(".ssh/id_ed25519"), "private");
        write(&fixture.env.home.join(".ssh/id_ed25519.pub"), "ssh-ed25519 AAAA me@laptop\n");
        let key = key(&fixture.env, None).unwrap();

        let published = ensure_recipient(&fixture.repo, &key).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&published).unwrap(), "ssh-ed25519 AAAA me@laptop\n");
        assert!(ensure_recipient(&fixture.repo, &key).unwrap().is_none());
        assert_eq!(recipients(&fixture.repo), [published]);
    }

    #[test]
    fn plaintext_is_private_and_decrypted_only_when_the_file_changes() {
        let fixture = fixture(&[]);
        let encrypted = fixture.repo.static_dir().join("rclone/rclone.conf.age");
        write(&encrypted, "token = 1\n");
        let (runner, mut inputs) = (age(), Inputs::default());

        let plain = plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut inputs, true).unwrap().plain;
        assert_eq!(plain, fixture.env.state_dir.join("secrets/rclone/rclone.conf"));
        assert_eq!(fs::metadata(&plain).unwrap().permissions().mode() & 0o777, 0o600);
        plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut inputs, true).unwrap();
        assert_eq!(runner.calls.borrow().len(), 1);

        assert_eq!(find(&fixture.env, &fixture.repo, &encrypted).unwrap(), encrypted);
        let live = fixture.env.home.join(".config/rclone/rclone.conf");
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&plain, &live).unwrap();
        assert_eq!(find(&fixture.env, &fixture.repo, &live).unwrap(), encrypted);
    }

    #[test]
    fn a_file_for_other_machines_says_how_to_get_it() {
        let fixture = fixture(&[]);
        let encrypted = fixture.repo.static_dir().join("x/foreign.age");
        write(&encrypted, "?");
        let error = plaintext(&fixture.env, &age(), &fixture.repo, Path::new("/k"), &encrypted, &mut Inputs::default(), true).unwrap_err();
        assert!(matches!(error, SecretsError::NotRecipient(file, host) if file == "static/x/foreign.age" && host == "host"));
    }

    #[test]
    fn a_wrong_passphrase_is_an_error_and_retried_next_time() {
        let fixture = fixture(&[]);
        let encrypted = fixture.repo.static_dir().join("x/locked.age");
        write(&encrypted, "?");
        let runner = age();
        let mut inputs = Inputs::default();
        assert!(matches!(plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut inputs, true), Err(SecretsError::Run(_))));
        assert!(plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut inputs, true).is_err());
        assert_eq!(runner.calls.borrow().len(), 2);
    }

    #[test]
    fn a_dry_run_never_decrypts() {
        let fixture = fixture(&[]);
        let encrypted = fixture.repo.static_dir().join("x/token.age");
        write(&encrypted, "secret\n");
        let runner = age();
        assert!(matches!(plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut Inputs::default(), false), Err(SecretsError::Pending)));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn an_edited_copy_is_backed_up_before_a_new_decrypt() {
        let fixture = fixture(&[]);
        let encrypted = fixture.repo.static_dir().join("x/token.age");
        write(&encrypted, "one\n");
        let runner = age();
        let plain = plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut Inputs::default(), true).unwrap().plain;

        // edited through the link, then the encrypted file changes
        fs::write(&plain, "my edit\n").unwrap();
        write(&encrypted, "two\n");
        let placed = plaintext(&fixture.env, &runner, &fixture.repo, Path::new("/k"), &encrypted, &mut Inputs::default(), true).unwrap();
        assert_eq!(fs::read_to_string(&placed.plain).unwrap(), "two\n");
        assert_eq!(fs::read_to_string(placed.backup.unwrap()).unwrap(), "my edit\n");
    }
}
