use std::fs;
use std::path::Path;

// committed files maw generates must match what it generates now; MAW_BLESS=1 rewrites them
fn check(dir: &str, files: Vec<(String, String)>) {
    files.iter().for_each(|(name, content)| {
        let path = Path::new(dir).join(name);
        if std::env::var_os("MAW_BLESS").is_some() {
            fs::create_dir_all(dir).unwrap();
            fs::write(&path, content).unwrap();
        }
        let committed = fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}; run MAW_BLESS=1 cargo test", path.display()));
        assert_eq!(&committed, content, "{} is stale; run MAW_BLESS=1 cargo test", path.display());
    });
}

#[test]
fn man_pages_are_current() {
    check("man", maw::man::pages());
}

#[test]
fn completion_scripts_are_current() {
    check("completions", maw::complete::scripts());
}
