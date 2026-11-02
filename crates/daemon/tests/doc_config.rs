//! `docs/configuration.md` must show the real defaults and name every field.

use std::path::PathBuf;

fn doc() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/configuration.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn documented_json(text: &str) -> serde_json::Value {
    let start = text
        .find("```json")
        .expect("configuration.md needs a json block")
        + "```json".len();
    let rest = &text[start..];
    let end = rest.find("```").expect("unterminated json block");
    serde_json::from_str(&rest[..end]).expect("documented JSON must parse")
}

#[test]
fn documented_defaults_match_the_code() {
    let documented = documented_json(&doc());
    let actual = serde_json::to_value(daemon::Config::default()).unwrap();
    assert_eq!(
        documented, actual,
        "docs/configuration.md drifted from Config::default()"
    );
}

#[test]
fn every_field_is_documented() {
    let text = doc();
    let actual = serde_json::to_value(daemon::Config::default()).unwrap();
    for (key, value) in actual.as_object().unwrap() {
        assert!(
            text.contains(&format!("`{key}`")),
            "missing table row for `{key}`"
        );
        // `keybindings` maps actions to chords and is documented in its own table.
        if key != "keybindings" {
            if let Some(nested) = value.as_object() {
                for nested_key in nested.keys() {
                    assert!(
                        text.contains(&format!("`{}.{}`", key, nested_key)),
                        "missing table row for `{key}.{nested_key}`"
                    );
                }
            }
        }
    }
    for action in actual["keybindings"].as_object().unwrap().keys() {
        assert!(
            text.contains(&format!("`{action}`")),
            "missing keybinding row for `{action}`"
        );
    }
}
