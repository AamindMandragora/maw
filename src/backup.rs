use crate::env::Env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// where a path is mirrored: relative to home, or under system/ for anything outside it
fn mirror(env: &Env, path: &Path) -> PathBuf {
    match path.strip_prefix(&env.home) {
        Ok(relative) => env.backup_dir().join(relative),
        Err(_) => env.backup_dir().join("system").join(path.strip_prefix("/").unwrap_or(path)),
    }
}

// moves a file or link into the backup mirror, numbering it if an older backup exists; returns where it went
pub fn backup(env: &Env, path: &Path) -> io::Result<PathBuf> {
    let mirror = mirror(env, path);
    let taken = |candidate: &PathBuf| fs::symlink_metadata(candidate).is_ok();

    // backups/x, then backups/x.1, backups/x.2, ...
    let target = std::iter::once(mirror.clone())
        .chain((1..).map(|number| PathBuf::from(format!("{}.{number}", mirror.display()))))
        .find(|candidate| !taken(candidate))
        .unwrap();

    fs::create_dir_all(target.parent().unwrap())?;
    fs::rename(path, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_mirror_the_path_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(&dir.path().join("home"), Path::new("/"), Path::new("/s"));
        let file = env.home.join(".config/foot/foot.ini");
        fs::create_dir_all(file.parent().unwrap()).unwrap();

        fs::write(&file, "one").unwrap();
        let first = backup(&env, &file).unwrap();
        fs::write(&file, "two").unwrap();
        let second = backup(&env, &file).unwrap();

        assert_eq!(first, env.backup_dir().join(".config/foot/foot.ini"));
        assert_eq!(second.file_name().unwrap(), "foot.ini.1");
        assert_eq!(fs::read_to_string(first).unwrap(), "one");
        assert!(!file.exists());
    }

    #[test]
    fn paths_outside_home_mirror_under_system() {
        let env = Env::new(Path::new("/h"), Path::new("/"), Path::new("/s"));
        assert_eq!(mirror(&env, Path::new("/etc/greetd/config.toml")), env.backup_dir().join("system/etc/greetd/config.toml"));
    }
}
