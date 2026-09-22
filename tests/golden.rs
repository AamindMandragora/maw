use serde_json::Value;
use std::{fs, path::Path, process::Command};

// evaluates a nix file with <maw> pointed at ./nix and returns its json
fn eval(args: &[&str]) -> Value {
    let output = Command::new("nix-instantiate")
        .args(["--eval", "--strict", "--json", "-I", "maw=nix"])
        .args(args)
        .output()
        .expect("nix-instantiate on PATH");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

// compares text to a golden file, rewriting the golden first when MAW_BLESS is set
fn check(golden: &Path, actual: &str) {
    if std::env::var_os("MAW_BLESS").is_some() {
        fs::create_dir_all(golden.parent().unwrap()).unwrap();
        fs::write(golden, actual).unwrap();
    }

    let expected = fs::read_to_string(golden).unwrap_or_else(|_| panic!("missing golden {}", golden.display()));
    assert_eq!(actual, expected, "{}", golden.display());
}

// file stems of every .nix file in a directory
fn nix_stems(dir: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "nix"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn generators_match_goldens() {
    // tests/generators/<name>.nix evaluates to a string matching <name>.out
    nix_stems("tests/generators").iter().for_each(|name| {
        let rendered = eval(&[&format!("tests/generators/{name}.nix")]);
        check(&Path::new("tests/generators").join(format!("{name}.out")), rendered.as_str().unwrap());
    });
}

#[test]
fn modules_match_goldens() {
    // each file a fixture module renders matches tests/golden/<module>/<key>
    nix_stems("tests/fixtures/dotfiles/modules").iter().for_each(|name| {
        let files = eval(&["-A", &format!("modules.{name}"), "tests/fixtures/dotfiles"]);
        files.as_array().unwrap().iter().for_each(|file| {
            let golden = Path::new("tests/golden").join(name).join(file["key"].as_str().unwrap());
            check(&golden, file["content"].as_str().unwrap());
        });
    });
}
