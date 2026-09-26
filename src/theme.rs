use crate::env::Env;
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

// a palette generated from a wallpaper with matugen, kept in the state dir as machine state rather than repo history,
// and handed to config.nix and every module as `theme`

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("matugen isn't installed; `maw install matugen` first")]
    NoMatugen,
    #[error("matugen couldn't read {0}")]
    Unreadable(String),
    #[error("static/wallpapers/ has no images; `maw wallpaper <image>` adds one")]
    NoWallpapers,
    #[error("static/wallpapers/{0} is already a different image; rename yours")]
    Taken(String),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ThemeError + '_ {
    move |source| ThemeError::Io { path: path.into(), source }
}

// maw.theme in config.nix: how palettes are made
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    // dark or light
    pub mode: String,
    // matugen's scheme type, e.g. scheme-tonal-spot or scheme-vibrant
    pub scheme: String,
    // -1 to 1, 0 being the standard
    pub contrast: Option<f64>,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        ThemeSettings { mode: "dark".into(), scheme: "scheme-tonal-spot".into(), contrast: None }
    }
}

// the current theme, as config.nix and modules see it
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    // the wallpaper relative to the repo, and absolute for configs to point at
    pub image: String,
    pub wallpaper: String,
    pub settings: ThemeSettings,
    // which of the image's candidate colors the palette grew from
    pub index: usize,
    pub count: usize,
    pub source: String,
    // material colors and base16 colors in the chosen mode, as #rrggbb
    pub colors: BTreeMap<String, String>,
    pub base16: BTreeMap<String, String>,
}

pub fn file(env: &Env) -> PathBuf {
    env.state_dir.join("theme.json")
}

pub fn load(env: &Env) -> Option<Theme> {
    serde_json::from_str(&fs::read_to_string(file(env)).ok()?).ok()
}

fn save(env: &Env, theme: &Theme) -> Result<(), ThemeError> {
    let path = file(env);
    fs::create_dir_all(&env.state_dir).map_err(io(&env.state_dir))?;
    fs::write(&path, serde_json::to_string_pretty(theme).unwrap_or_default()).map_err(io(&path))
}

fn wallpapers_dir(repo: &Repo) -> PathBuf {
    repo.static_dir().join("wallpapers")
}

// images in static/wallpapers/, by name
pub fn wallpapers(repo: &Repo) -> Vec<String> {
    let images = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"];
    let entries = fs::read_dir(wallpapers_dir(repo)).into_iter().flatten().filter_map(|entry| entry.ok()).map(|entry| entry.path());
    let mut names: Vec<String> = entries
        .filter(|path| path.extension().is_some_and(|ext| images.contains(&ext.to_string_lossy().to_lowercase().as_str())))
        .filter_map(|path| Some(path.file_name()?.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

// a random number below bound, from the hasher seed std draws per process
pub fn random(bound: usize) -> usize {
    let seed = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (seed % bound.max(1) as u64) as usize
}

fn matugen(runner: &dyn Runner, args: &[String]) -> Result<String, ThemeError> {
    runner.run("matugen", args).map_err(|error| match error {
        RunError::Spawn { .. } => ThemeError::NoMatugen,
        error => error.into(),
    })
}

// the colors an image offers to grow a palette from, most dominant first
fn candidates(runner: &dyn Runner, image: &Path) -> Result<usize, ThemeError> {
    let output = matugen(runner, &["image".into(), image.display().to_string(), "--show-source-colors".into()])?;
    Ok(output.lines().filter(|line| line.trim().starts_with('#')).count().max(1))
}

// a mode's colors from matugen's json: { name: { dark: { color }, light: { color } } }
fn mode_colors(section: &Value, mode: &str) -> BTreeMap<String, String> {
    let entries = section.as_object().into_iter().flatten();
    entries.filter_map(|(name, modes)| Some((name.clone(), modes[mode]["color"].as_str()?.to_string()))).collect()
}

// a palette from static/wallpapers/<name>, grown from the candidate color at index, or a random one
pub fn generate(runner: &dyn Runner, repo: &Repo, name: &str, settings: &ThemeSettings, index: Option<usize>) -> Result<Theme, ThemeError> {
    let image = wallpapers_dir(repo).join(name);
    let count = candidates(runner, &image)?;
    let index = index.unwrap_or_else(|| random(count)).min(count - 1);

    let mut args: Vec<String> = ["image", &image.display().to_string(), "-m", &settings.mode, "-t", &settings.scheme, "--source-color-index", &index.to_string()].map(String::from).to_vec();
    args.extend(settings.contrast.map(|contrast| ["--contrast".to_string(), contrast.to_string()]).into_iter().flatten());
    args.extend(["--json", "hex", "--dry-run", "-q"].map(String::from));
    let json: Value = serde_json::from_str(&matugen(runner, &args)?).map_err(|_| ThemeError::Unreadable(name.into()))?;

    let colors = mode_colors(&json["colors"], &settings.mode);
    Ok(Theme {
        image: format!("static/wallpapers/{name}"),
        wallpaper: image.display().to_string(),
        settings: settings.clone(),
        index,
        count,
        source: colors.get("source_color").cloned().unwrap_or_default(),
        base16: mode_colors(&json["base16"], &settings.mode),
        colors,
    })
}

// an image as static/wallpapers/<name>, copied in unless it's there already; true if copied
pub fn adopt(repo: &Repo, image: &Path) -> Result<(String, bool), ThemeError> {
    let name = image.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let target = wallpapers_dir(repo).join(&name);
    if image.canonicalize().ok() == target.canonicalize().ok() && target.exists() {
        return Ok((name, false));
    }
    let bytes = fs::read(image).map_err(io(image))?;
    match fs::read(&target) {
        Ok(existing) if existing == bytes => Ok((name, false)),
        Ok(_) => Err(ThemeError::Taken(name)),
        Err(_) => {
            fs::create_dir_all(wallpapers_dir(repo)).map_err(io(&target))?;
            fs::write(&target, bytes).map_err(io(&target))?;
            Ok((name, true))
        }
    }
}

// sets the theme to a wallpaper, with a random candidate color
pub fn set(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, settings: &ThemeSettings) -> Result<Theme, ThemeError> {
    let theme = generate(runner, repo, name, settings, None)?;
    save(env, &theme)?;
    Ok(theme)
}

// a random wallpaper other than the current one, when there's a choice
pub fn pick_random(env: &Env, repo: &Repo) -> Result<String, ThemeError> {
    let current = load(env).map(|theme| theme.image);
    let names = wallpapers(repo);
    let others: Vec<&String> = names.iter().filter(|name| Some(format!("static/wallpapers/{name}")) != current).collect();
    let pool: Vec<&String> = if others.is_empty() { names.iter().collect() } else { others };
    pool.get(random(pool.len())).map(|name| name.to_string()).ok_or(ThemeError::NoWallpapers)
}

// keeps the theme in step with config.nix before a build: made from the first wallpaper when maw.theme asks for one and
// there's none yet, and regenerated from the same wallpaper and color when the settings change
pub fn ensure(env: &Env, runner: &dyn Runner, repo: &Repo, settings: Option<&ThemeSettings>) -> Result<(), ThemeError> {
    let current = load(env).filter(|theme| repo.root.join(&theme.image).exists());
    let wanted = settings.cloned().or_else(|| current.as_ref().map(|theme| theme.settings.clone()));
    let theme = match (current, wanted) {
        (Some(theme), Some(wanted)) if theme.settings == wanted => return Ok(()),
        (Some(theme), Some(wanted)) => generate(runner, repo, theme.image.trim_start_matches("static/wallpapers/"), &wanted, Some(theme.index))?,
        (None, Some(wanted)) => generate(runner, repo, wallpapers(repo).first().ok_or(ThemeError::NoWallpapers)?, &wanted, Some(0))?,
        (_, None) => return Ok(()),
    };
    save(env, &theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::testing::{fixture, write};

    const JSON: &str = r##"{"colors":{"primary":{"dark":{"color":"#a6d0b0"},"light":{"color":"#2f6a47"}},"source_color":{"dark":{"color":"#7f8d2e"},"light":{"color":"#7f8d2e"}}},"base16":{"base00":{"dark":{"color":"#0c2011"},"light":{"color":"#9abecd"}}}}"##;

    // matugen offering three candidate colors
    fn matugen() -> FakeRunner {
        FakeRunner::new(|_, args| match args.iter().any(|arg| arg == "--show-source-colors") {
            true => "#7f8d2e\n#7aaacd\n#93b1a5\n".into(),
            false => JSON.into(),
        })
    }

    #[test]
    fn a_palette_takes_the_modes_colors() {
        let fixture = fixture(&[]);
        write(&fixture.repo.static_dir().join("wallpapers/paper.jpg"), "jpeg");
        let runner = matugen();
        let theme = generate(&runner, &fixture.repo, "paper.jpg", &ThemeSettings::default(), Some(2)).unwrap();
        assert_eq!((theme.colors["primary"].as_str(), theme.base16["base00"].as_str(), theme.source.as_str()), ("#a6d0b0", "#0c2011", "#7f8d2e"));
        assert_eq!((theme.index, theme.count, theme.image.as_str()), (2, 3, "static/wallpapers/paper.jpg"));
        assert!(runner.calls.borrow().last().unwrap().contains("-m dark -t scheme-tonal-spot --source-color-index 2 --json hex --dry-run -q"));
    }

    #[test]
    fn ensure_starts_from_the_first_wallpaper_and_follows_settings() {
        let fixture = fixture(&[]);
        write(&fixture.repo.static_dir().join("wallpapers/b.png"), "png");
        write(&fixture.repo.static_dir().join("wallpapers/a.jpg"), "jpeg");
        let runner = matugen();

        // no maw.theme and no theme: nothing happens, and matugen is never run
        ensure(&fixture.env, &runner, &fixture.repo, None).unwrap();
        assert!(load(&fixture.env).is_none() && runner.calls.borrow().is_empty());

        ensure(&fixture.env, &runner, &fixture.repo, Some(&ThemeSettings::default())).unwrap();
        assert_eq!(load(&fixture.env).unwrap().image, "static/wallpapers/a.jpg");

        // same settings: untouched; light mode: regenerated from the same color
        let calls = runner.calls.borrow().len();
        ensure(&fixture.env, &runner, &fixture.repo, Some(&ThemeSettings::default())).unwrap();
        assert_eq!(runner.calls.borrow().len(), calls);
        let light = ThemeSettings { mode: "light".into(), ..ThemeSettings::default() };
        ensure(&fixture.env, &runner, &fixture.repo, Some(&light)).unwrap();
        assert_eq!(load(&fixture.env).unwrap().colors["primary"], "#2f6a47");
    }

    #[test]
    fn images_are_copied_in_once_and_never_over_another() {
        let fixture = fixture(&[]);
        let outside = fixture.dir.path().join("forest.jpg");
        write(&outside, "forest");
        assert_eq!(adopt(&fixture.repo, &outside).unwrap(), ("forest.jpg".to_string(), true));
        assert_eq!(adopt(&fixture.repo, &outside).unwrap(), ("forest.jpg".to_string(), false));
        write(&outside, "another forest");
        assert!(matches!(adopt(&fixture.repo, &outside), Err(ThemeError::Taken(_))));
    }

    #[test]
    fn without_matugen_the_error_says_what_to_install() {
        let fixture = fixture(&[]);
        write(&fixture.repo.static_dir().join("wallpapers/a.jpg"), "a");
        let runner = FakeRunner::fallible(|program, _| Err(RunError::Spawn { program: program.into(), source: std::io::ErrorKind::NotFound.into() }));
        assert!(matches!(set(&fixture.env, &runner, &fixture.repo, "a.jpg", &ThemeSettings::default()), Err(ThemeError::NoMatugen)));
    }

    #[test]
    fn random_picks_another_wallpaper_when_there_is_one() {
        let fixture = fixture(&[]);
        write(&fixture.repo.static_dir().join("wallpapers/a.jpg"), "a");
        write(&fixture.repo.static_dir().join("wallpapers/b.jpg"), "b");
        set(&fixture.env, &matugen(), &fixture.repo, "a.jpg", &ThemeSettings::default()).unwrap();
        assert!((0..10).all(|_| pick_random(&fixture.env, &fixture.repo).unwrap() == "b.jpg"));
        assert!(random(3) < 3);
    }
}
