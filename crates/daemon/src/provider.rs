//! Provider selection: helper → tmux → no persistence.
//!
//! Keeping this pure makes the policy testable without SSH, and forces the caller to
//! pass a reason for every unavailable option — the GUI shows those reasons instead of
//! silently downgrading.

use protocol::ProviderKind;

/// Why a provider cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub kind: ProviderKind,
    pub reason: String,
}

/// Result of the selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub kind: ProviderKind,
    /// Reasons for every provider that was skipped, in priority order.
    pub skipped: Vec<Unavailable>,
}

/// Pick the first available provider in priority order. `None` is always available, so
/// the function never fails; the caller reports `skipped` to the user.
pub fn select(available: &[(ProviderKind, Option<String>)]) -> Selection {
    let mut skipped = Vec::new();
    for (kind, blocked) in available {
        match blocked {
            None => {
                return Selection {
                    kind: *kind,
                    skipped,
                };
            }
            Some(reason) => skipped.push(Unavailable {
                kind: *kind,
                reason: reason.clone(),
            }),
        }
    }
    Selection {
        kind: ProviderKind::None,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_wins_when_available() {
        let selection = select(&[(ProviderKind::Helper, None), (ProviderKind::Tmux, None)]);
        assert_eq!(selection.kind, ProviderKind::Helper);
        assert!(selection.skipped.is_empty());
    }

    #[test]
    fn falls_back_to_tmux_and_keeps_the_reason() {
        let selection = select(&[
            (
                ProviderKind::Helper,
                Some("architecture aarch64 unsupported".into()),
            ),
            (ProviderKind::Tmux, None),
        ]);
        assert_eq!(selection.kind, ProviderKind::Tmux);
        assert_eq!(selection.skipped.len(), 1);
        assert!(selection.skipped[0].reason.contains("aarch64"));
    }

    #[test]
    fn falls_back_to_none_and_reports_every_skip() {
        let selection = select(&[
            (ProviderKind::Helper, Some("checksum mismatch".into())),
            (ProviderKind::Tmux, Some("tmux not installed".into())),
        ]);
        assert_eq!(selection.kind, ProviderKind::None);
        assert_eq!(selection.skipped.len(), 2);
        assert_eq!(selection.skipped[1].reason, "tmux not installed");
    }

    #[test]
    fn tmux_only_hosts_still_work() {
        let selection = select(&[(ProviderKind::Tmux, None)]);
        assert_eq!(selection.kind, ProviderKind::Tmux);
    }
}
