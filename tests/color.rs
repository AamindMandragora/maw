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

// negative turns wrap around the wheel, and lightness clamps at white and black
#[test]
fn color_wraps_hue_and_clamps_lightness() {
    assert_eq!(eval(r##"[ (lib.color.rotate (-480) "#ff0000") (lib.color.rotate (-360) "#9dcbfb") ]"##), serde_json::json!(["#0000ff", "#9dcbfb"]));
    assert_eq!(eval(r##"[ (lib.color.lighten 2 "#123456") (lib.color.lighten (-2) "123456") ]"##), serde_json::json!(["#ffffff", "000000"]));
}

// anything but six hex digits is refused by name
#[test]
fn color_rejects_bad_input() {
    ["\"zz\"", "\"#12345\"", "\"#1234567\"", "\"##123456\"", "5"].iter().for_each(|bad| {
        let program = format!("let lib = import ./nix/lib.nix; in lib.color.rotate 10 {bad}");
        let output = Command::new("nix-instantiate").args(["--eval", "--strict", "--json", "-E", &program]).output().unwrap();
        assert!(String::from_utf8_lossy(&output.stderr).contains("maw: lib.color wants #rrggbb, got"), "{bad}");
    });
}
