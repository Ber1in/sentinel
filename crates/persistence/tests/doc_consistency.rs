//! `docs/providers.md` must describe the options the code actually uses.

use std::path::PathBuf;

use persistence::TmuxOptions;

fn doc() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/providers.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn documented_tmux_options_match_the_defaults() {
    let text = doc();
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("options:"))
        .expect("providers.md must contain an `options:` line")
        .trim();

    let options = TmuxOptions::default();
    let expected = format!(
        "options: socket={} history-limit={} status={} mouse={} default-terminal={} destroy-unattached=off",
        options.socket,
        options.history_limit,
        if options.status { "on" } else { "off" },
        if options.mouse { "on" } else { "off" },
        options.default_terminal
    );
    assert_eq!(
        line, expected,
        "docs/providers.md drifted from TmuxOptions::default()"
    );
}

#[test]
fn documented_commands_exist_in_the_implementation() {
    let text = doc();
    let options = TmuxOptions::default();
    let provider = persistence::TmuxPersistence::new(options);

    for expected in [
        provider.probe_command(),
        provider.has_session_command("s"),
        provider.attach_command("s", Some("/tmp")),
        provider.snapshot_command("s", 10),
        provider.destroy_command("s"),
    ] {
        // Every command the provider emits must be describable by the document's rules:
        // check the stable, human-meaningful fragments rather than the whole string.
        let fragment = expected
            .split_whitespace()
            .find(|token| {
                token.starts_with("capture-pane")
                    || token.starts_with("has-session")
                    || token.starts_with("kill-session")
                    || token.starts_with("new-session")
                    || token.contains("command -v tmux")
            })
            .unwrap_or("tmux");
        assert!(
            text.contains(fragment),
            "providers.md does not mention `{fragment}`"
        );
    }
}
