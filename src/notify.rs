//! Desktop notifications. Child creation and reaping never run on the event-loop thread.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::layout::PaneId;
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};
use crate::sidebar::Notice;

pub(crate) fn unseen_notice(
    window_focused: bool,
    focused: Option<PaneId>,
    pane: PaneId,
    title: Option<&str>,
    body: &str,
) -> Option<Notice> {
    (!(window_focused && focused == Some(pane))).then(|| Notice::new(title, body))
}

#[derive(Default)]
pub struct RateLimiter {
    sent: HashMap<PaneId, Instant>,
}

impl RateLimiter {
    pub fn allow(&mut self, pane: PaneId, now: Instant) -> bool {
        if self
            .sent
            .get(&pane)
            .is_some_and(|at| now.saturating_duration_since(*at) < Duration::from_secs(3))
        {
            return false;
        }
        self.sent.insert(pane, now);
        true
    }

    pub fn forget(&mut self, pane: PaneId) {
        self.sent.remove(&pane);
    }
}

#[derive(Default)]
pub struct DesktopNotifier {
    limiter: RateLimiter,
}

impl DesktopNotifier {
    pub(crate) fn show(
        &mut self,
        enabled: bool,
        pane: PaneId,
        notice: &Notice,
        tab_title: impl FnOnce() -> String,
    ) {
        self.show_with(enabled, pane, notice, tab_title, show);
    }

    fn show_with(
        &mut self,
        enabled: bool,
        pane: PaneId,
        notice: &Notice,
        tab_title: impl FnOnce() -> String,
        send: impl FnOnce(String, String),
    ) {
        if enabled && self.limiter.allow(pane, Instant::now()) {
            let title = notice.title.clone().unwrap_or_else(|| {
                let title = clean_text(&tab_title(), MAX_NOTIFY_CHARS);
                if title.is_empty() {
                    "Twitty".into()
                } else {
                    title
                }
            });
            send(title, notice.body.clone());
        }
    }

    pub fn forget(&mut self, pane: PaneId) {
        self.limiter.forget(pane);
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
pub fn command(title: &str, body: &str) -> std::process::Command {
    let title = clean_text(title, MAX_NOTIFY_CHARS);
    let body = clean_text(body, MAX_NOTIFY_CHARS);
    let mut command = std::process::Command::new("notify-send");
    command
        .args(["--app-name=Twitty", "--", &title, &body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn show(title: String, body: String) {
    if let Err(error) = std::thread::Builder::new()
        .name("twitty-notify".into())
        .spawn(move || match command(&title, &body).spawn() {
            Ok(mut child) => {
                if let Err(error) = child.wait() {
                    tracing::debug!(%error, "notify-send wait failed");
                }
            }
            Err(error) => tracing::debug!(%error, "desktop notification unavailable"),
        })
    {
        tracing::debug!(%error, "desktop notification thread unavailable");
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn show(_title: String, _body: String) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_notifications_are_ignored_and_disabled_delivery_leaves_attention_available() {
        let target = PaneId(1);
        assert!(unseen_notice(true, Some(target), target, None, "done").is_none());
        assert!(unseen_notice(false, Some(target), target, None, "done").is_some());
        let notice = unseen_notice(
            true,
            Some(PaneId(2)),
            target,
            Some("\u{061c}"),
            "تم\u{200f}",
        )
        .expect("another pane asked for attention");
        let mut notifier = DesktopNotifier::default();
        notifier.show_with(
            false,
            target,
            &notice,
            || panic!("unused title"),
            |_, _| panic!("disabled notification"),
        );
        assert_eq!(notice.label(), "تم");
        let mut delivered = vec![];
        notifier.show_with(
            true,
            target,
            &notice,
            || "محرر\u{202e}".into(),
            |title, body| delivered.push((title, body)),
        );
        notifier.show_with(
            true,
            target,
            &notice,
            || panic!("throttled title"),
            |_, _| panic!("throttled notification"),
        );
        assert_eq!(delivered, [("محرر".into(), "تم".into())]);
    }
}
