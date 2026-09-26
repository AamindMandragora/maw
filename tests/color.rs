use std::process::Command;

// evaluates an expression over maw's lib with nix, as json
fn eval(expression: &str) -> serde_json::Value {
    let program = format!("let lib = import ./nix/lib.nix; in {expression}");
    let output = Command::new("nix-instantiate").args(["--eval", "--strict", "--json", "-E", &program]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

// colors survive the trip through hsl, and hue turns land where they should
#[test]
fn color_math_is_exact_on_known_colors() {
    let round = eval(r##"map (lib.color.rotate 0) [ "#9dcbfb" "#ffb4ab" "101418" "#ffffff" "#000000" "#808080" ]"##);
    assert_eq!(round, serde_json::json!(["#9dcbfb", "#ffb4ab", "101418", "#ffffff", "#000000", "#808080"]));
    assert_eq!(eval(r##"[ (lib.color.rotate 120 "#ff0000") (lib.color.rotate (-120) "#ff0000") (lib.color.rotate 480 "ff0000") ]"##), serde_json::json!(["#00ff00", "#0000ff", "00ff00"]));
    assert_eq!(eval(r##"lib.color.lighten 0.5 "#000000""##), "#808080");
    assert_eq!(eval(r##"lib.color.ansi "#9dcbfb""##), "38;2;157;203;251");
}
