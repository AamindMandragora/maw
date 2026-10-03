use crate::backend;
use crate::env::Env;
use crate::runner::Runner;
use crate::state::MawState;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// problems that break a desktop quietly, each with the fix: things maw can see but doesn't manage itself

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub problem: String,
    pub fix: String,
}

fn finding(problem: impl Into<String>, fix: impl Into<String>) -> Finding {
    Finding { problem: problem.into(), fix: fix.into() }
}

// every check; vars is the environment maw runs in, state what this machine declares (None outside a repo)
pub fn check(env: &Env, runner: &dyn Runner, vars: &BTreeMap<String, String>, state: Option<&MawState>) -> Vec<Finding> {
    let greetd = greetd_config(env);
    let checks = [
        session_bus(vars),
        greeter_bus(env, greetd.as_deref()),
        greetd_vt(greetd.as_deref()),
        keyring_pam(env),
        electron_hint(vars),
        turnstile(env),
    ];
    let path = state.map(|state| off_path(env, runner, vars, state)).unwrap_or_default();
    checks.into_iter().flatten().chain(path).collect()
}

// greetd's config, when greetd is enabled
fn greetd_config(env: &Env) -> Option<String> {
    let enabled = env.sysroot.join("var/service/greetd").exists();
    enabled.then(|| fs::read_to_string(env.sysroot.join("etc/greetd/config.toml")).unwrap_or_default())
}

// apps find the keyring and each other on the session bus; without a shared one each starts its own
// apps find the keyring and each other on the session bus: without a shared one each starts its own, and without its
// address in the environment chromium apps (vivaldi's key store) can't find it even when it's there
fn session_bus(vars: &BTreeMap<String, String>) -> Option<Finding> {
    let address = vars.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str).unwrap_or_default();
    let socket = address.strip_prefix("unix:path=").map(|path| PathBuf::from(path.split(',').next().unwrap_or(path)));
    let default = vars.get("XDG_RUNTIME_DIR").map(|dir| Path::new(dir).join("bus")).filter(|socket| socket.exists());
    match (socket.is_some_and(|socket| socket.exists()), default) {
        (true, _) => None,
        (false, Some(socket)) => Some(finding("DBUS_SESSION_BUS_ADDRESS isn't set", format!("set it to \"unix:path={}\" in your compositor's environment", socket.display()))),
        (false, None) => Some(finding("no shared session bus", "declare a core dbus user service: `maw help modules`, under lib.service")),
    }
}

// dbus-run-session gives the compositor a private bus, apart from the keyring's and the user services'
fn greeter_bus(env: &Env, greetd: Option<&str>) -> Option<Finding> {
    let config = greetd?;
    let sessions = fs::read_dir(env.sysroot.join("usr/share/wayland-sessions")).into_iter().flatten().filter_map(|entry| fs::read_to_string(entry.ok()?.path()).ok());
    let through_sessions = config.contains("--sessions") && sessions.into_iter().any(|desktop| desktop.contains("dbus-run-session"));
    (config.contains("dbus-run-session") || through_sessions).then(|| finding("greetd starts your session on a private bus (dbus-run-session)", "start the compositor itself: `tuigreet --cmd 'niri --session'`"))
}

// greetd needs a vt to run on
fn greetd_vt(greetd: Option<&str>) -> Option<Finding> {
    let config = greetd?;
    let has_vt = config.lines().any(|line| line.trim_start().starts_with("vt") || line.contains("terminal.vt"));
    (!has_vt).then(|| finding("/etc/greetd/config.toml has no vt", "add `terminal.vt = 7` to the greetd module"))
}

// the keyring unlocks with the login password only when the login's pam stack hands it over
fn keyring_pam(env: &Env) -> Option<Finding> {
    if !env.sysroot.join("usr/bin/gnome-keyring-daemon").exists() {
        return None;
    }
    let login = if env.sysroot.join("var/service/greetd").exists() { "greetd" } else { "login" };
    let pam = fs::read_to_string(env.sysroot.join("etc/pam.d").join(login)).unwrap_or_default();
    let active = |kind: &str| pam.lines().any(|line| !line.trim_start().starts_with('#') && line.trim_start().trim_start_matches('-').starts_with(kind) && line.contains("pam_gnome_keyring.so"));
    (!active("auth") || !active("session")).then(|| {
        let fix = "add `-auth optional pam_gnome_keyring.so` and `-session optional pam_gnome_keyring.so auto_start`";
        finding(format!("/etc/pam.d/{login} doesn't unlock the keyring"), fix)
    })
}

// electron apps look for an x server unless told about wayland
fn electron_hint(vars: &BTreeMap<String, String>) -> Option<Finding> {
    let wayland = vars.contains_key("WAYLAND_DISPLAY");
    (wayland && !vars.contains_key("ELECTRON_OZONE_PLATFORM_HINT")).then(|| finding("electron apps (slack, vscode) won't use wayland", "set ELECTRON_OZONE_PLATFORM_HINT = \"auto\" in your compositor's environment"))
}

// user services run under turnstile
fn turnstile(env: &Env) -> Option<Finding> {
    let user_services = env.home.join(".config/service").is_dir();
    (user_services && !env.sysroot.join("var/service/turnstiled").exists()).then(|| finding("turnstiled isn't enabled, so user services don't run", "maw sv enable turnstiled"))
}

// bin dirs of sources with declared programs, missing from PATH
fn off_path(env: &Env, runner: &dyn Runner, vars: &BTreeMap<String, String>, state: &MawState) -> Vec<Finding> {
    let path: Vec<PathBuf> = std::env::split_paths(vars.get("PATH").map(String::as_str).unwrap_or_default()).collect();
    let used = state.packages.iter().filter(|(_, specs)| !specs.is_empty()).filter_map(|(name, _)| backend::for_name(name, runner, env)?.bin_dir());
    let missing: BTreeSet<PathBuf> = used.filter(|dir| !path.contains(dir)).collect();
    missing.into_iter().map(|dir| finding(format!("{} isn't on PATH", env.pretty(&dir)), format!("add `export PATH=\"{}:$PATH\"` to your shell profile", env.pretty(&dir).replace("~/", "$HOME/")))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::testing::write;

    fn setup() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), &dir.path().join("share"));
        (dir, env)
    }

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
    }

    // a desktop with every problem from the list: greetd through niri.desktop, a keyring pam can't unlock
    fn broken(env: &Env) {
        fs::create_dir_all(env.sysroot.join("var/service/greetd")).unwrap();
        write(&env.sysroot.join("etc/greetd/config.toml"), "[default_session]\ncommand = \"tuigreet --sessions /usr/share/wayland-sessions\"\n");
        write(&env.sysroot.join("usr/share/wayland-sessions/niri.desktop"), "Exec=dbus-run-session niri --session\n");
        write(&env.sysroot.join("usr/bin/gnome-keyring-daemon"), "");
        write(&env.sysroot.join("etc/pam.d/greetd"), "auth include system-local-login\n#-auth optional pam_gnome_keyring.so\n");
        fs::create_dir_all(env.home.join(".config/service")).unwrap();
    }

    #[test]
    fn a_broken_desktop_gets_every_finding() {
        let (_dir, env) = setup();
        broken(&env);
        let found = check(&env, &FakeRunner::new(|_, _| String::new()), &vars(&[("WAYLAND_DISPLAY", "wayland-1")]), None);
        let problems: Vec<&str> = found.iter().map(|finding| finding.problem.as_str()).collect();
        assert_eq!(problems, [
            "no shared session bus",
            "greetd starts your session on a private bus (dbus-run-session)",
            "/etc/greetd/config.toml has no vt",
            "/etc/pam.d/greetd doesn't unlock the keyring",
            "electron apps (slack, vscode) won't use wayland",
            "turnstiled isn't enabled, so user services don't run",
        ]);
    }

    #[test]
    fn a_working_desktop_is_clean() {
        let (dir, env) = setup();
        broken(&env);
        write(&env.sysroot.join("etc/greetd/config.toml"), "[default_session]\ncommand = \"tuigreet --cmd 'niri --session'\"\n\n[terminal]\nvt = 7\n");
        write(&env.sysroot.join("etc/pam.d/greetd"), "-auth optional pam_gnome_keyring.so\n-session optional pam_gnome_keyring.so auto_start\n");
        fs::create_dir_all(env.sysroot.join("var/service/turnstiled")).unwrap();
        let bus = dir.path().join("bus");
        write(&bus, "");
        let session = vars(&[("WAYLAND_DISPLAY", "wayland-1"), ("ELECTRON_OZONE_PLATFORM_HINT", "auto"), ("DBUS_SESSION_BUS_ADDRESS", &format!("unix:path={}", bus.display()))]);
        assert_eq!(check(&env, &FakeRunner::new(|_, _| String::new()), &session, None), []);

        // the bus is there, but nothing says where
        let unset = vars(&[("WAYLAND_DISPLAY", "wayland-1"), ("ELECTRON_OZONE_PLATFORM_HINT", "auto"), ("XDG_RUNTIME_DIR", &dir.path().display().to_string())]);
        let found = check(&env, &FakeRunner::new(|_, _| String::new()), &unset, None);
        assert_eq!(found, [finding("DBUS_SESSION_BUS_ADDRESS isn't set", format!("set it to \"unix:path={}\" in your compositor's environment", bus.display()))]);
    }

    #[test]
    fn declared_sources_need_their_bin_dir_on_path() {
        let (_dir, env) = setup();
        let state = MawState { packages: BTreeMap::from([("cargo".into(), vec!["bat".into()]), ("go".into(), Vec::new())]), ..MawState::default() };
        let runner = FakeRunner::new(|_, _| String::new());
        let found = off_path(&env, &runner, &vars(&[("PATH", "/usr/bin")]), &state);
        assert_eq!(found, [finding("~/.cargo/bin isn't on PATH", "add `export PATH=\"$HOME/.cargo/bin:$PATH\"` to your shell profile")]);
        assert!(off_path(&env, &runner, &vars(&[("PATH", &env.home.join(".cargo/bin").display().to_string())]), &state).is_empty());
    }
}
