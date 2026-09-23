use super::{InitBackend, Scope, ServiceDef};
use crate::env::Env;
use crate::runner::{RunError, Runner, as_root};
use std::path::{Path, PathBuf};

// void's init: definitions in /etc/sv or ~/.config/sv, enabled by links in /var/service or turnstile's ~/.config/service
pub struct Runit;

// a shell word in single quotes
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

// the run script: the module's shebang (or /bin/sh), stderr joined to the log, env exported, then the module's script
fn run_script(service: &ServiceDef) -> String {
    let (shebang, body) = match service.run.strip_prefix("#!") {
        Some(rest) => rest.split_once('\n').map_or((rest, ""), |(line, body)| (line, body)),
        None => ("/bin/sh", service.run.as_str()),
    };
    let exports: String = service.env.iter().map(|(key, value)| format!("export {key}={}\n", quote(value))).collect();
    format!("#!{shebang}\nexec 2>&1\n{exports}{}\n", body.trim_matches('\n'))
}

// log/run: svlogd into a dir it creates, timestamped and rotated
fn log_script(dir: &str) -> String {
    format!("#!/bin/sh\nmkdir -p {dir}\nexec svlogd -tt {dir}\n")
}

impl InitBackend for Runit {
    fn render(&self, name: &str, scope: Scope, service: &ServiceDef) -> Vec<(String, String, bool)> {
        let log_dir = match scope {
            Scope::System => format!("/var/log/{name}"),
            Scope::User => format!("\"$HOME/.local/state/log/{name}\""),
        };
        let log = service.log.then(|| ("log/run".to_string(), log_script(&log_dir), true));
        std::iter::once(("run".to_string(), run_script(service), true)).chain(log).collect()
    }

    fn definition(&self, env: &Env, scope: Scope, name: &str) -> PathBuf {
        match scope {
            Scope::System => env.sysroot.join("etc/sv").join(name),
            Scope::User => env.home.join(".config/sv").join(name),
        }
    }

    fn enabled_link(&self, env: &Env, scope: Scope, name: &str) -> PathBuf {
        match scope {
            Scope::System => env.sysroot.join("var/service").join(name),
            Scope::User => env.home.join(".config/service").join(name),
        }
    }

    fn log_file(&self, env: &Env, scope: Scope, name: &str) -> PathBuf {
        match scope {
            Scope::System => env.sysroot.join("var/log").join(name).join("current"),
            Scope::User => env.home.join(".local/state/log").join(name).join("current"),
        }
    }

    // sv on the enabled link; system services go through sudo
    fn control(&self, env: &Env, runner: &dyn Runner, scope: Scope, name: &str, action: &str) -> Result<(), RunError> {
        let args = [action.to_string(), self.enabled_link(env, scope, name).display().to_string()];
        match scope {
            Scope::System => as_root(runner, &env.sysroot, "sv", &args),
            Scope::User => runner.interactive("sv", &args),
        }
    }

    // sv status's first word; system services only when sudo needs no password
    fn state(&self, env: &Env, runner: &dyn Runner, scope: Scope, name: &str) -> Option<String> {
        let link = self.enabled_link(env, scope, name).display().to_string();
        let output = match scope {
            Scope::System if env.sysroot == Path::new("/") => runner.run("sudo", &["-n".into(), "sv".into(), "status".into(), link]),
            _ => runner.run("sv", &["status".into(), link]),
        };
        Some(output.ok()?.split(':').next()?.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use std::collections::BTreeMap;

    fn service(run: &str) -> ServiceDef {
        ServiceDef { run: run.into(), log: true, enable: true, env: BTreeMap::from([("A".into(), "it's".into())]) }
    }

    #[test]
    fn run_joins_stderr_and_exports_env() {
        let files = Runit.render("drive", Scope::User, &service("exec rclone mount"));
        assert_eq!(files[0], ("run".into(), "#!/bin/sh\nexec 2>&1\nexport A='it'\\''s'\nexec rclone mount\n".into(), true));
        assert_eq!(files[1].1, "#!/bin/sh\nmkdir -p \"$HOME/.local/state/log/drive\"\nexec svlogd -tt \"$HOME/.local/state/log/drive\"\n");
    }

    #[test]
    fn a_scripts_own_shebang_is_kept() {
        let files = Runit.render("x", Scope::System, &ServiceDef { log: false, ..service("#!/bin/bash\nset -e\nexec x") });
        assert_eq!(files.len(), 1);
        assert!(files[0].1.starts_with("#!/bin/bash\nexec 2>&1\nexport A="));
        assert!(files[0].1.ends_with("set -e\nexec x\n"));
    }

    #[test]
    fn paths_follow_scope() {
        let env = Env::new(Path::new("/h"), Path::new("/r"), Path::new("/s"));
        assert_eq!(Runit.definition(&env, Scope::System, "greetd"), PathBuf::from("/r/etc/sv/greetd"));
        assert_eq!(Runit.enabled_link(&env, Scope::User, "drive"), PathBuf::from("/h/.config/service/drive"));
        assert_eq!(Runit.log_file(&env, Scope::System, "greetd"), PathBuf::from("/r/var/log/greetd/current"));
    }

    #[test]
    fn system_control_uses_sudo_on_the_real_root() {
        let runner = FakeRunner::new(|_, _| "run: /var/service/x: (pid 1) 5s".into());
        let real = Env::new(Path::new("/h"), Path::new("/"), Path::new("/s"));
        Runit.control(&real, &runner, Scope::System, "x", "restart").unwrap();
        Runit.control(&real, &runner, Scope::User, "y", "down").unwrap();
        assert_eq!(*runner.calls.borrow(), ["sudo sv restart /var/service/x", "sv down /h/.config/service/y"]);
        assert_eq!(Runit.state(&real, &runner, Scope::System, "x").as_deref(), Some("run"));
    }
}
