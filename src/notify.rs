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
    if window_focused && focused == Some(pane) {
        return None;
    }
    let notice = Notice::new(title, body);
    (notice.title.is_some() || !notice.body.is_empty()).then_some(notice)
}

const THROTTLE_WINDOW: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct RateLimiter {
    sent: HashMap<PaneId, Instant>,
}

impl RateLimiter {
    pub fn allow(&mut self, pane: PaneId, now: Instant) -> bool {
        self.sent
            .retain(|_, at| now.saturating_duration_since(*at) < THROTTLE_WINDOW);
        if self.sent.contains_key(&pane) {
            return false;
        }
        self.sent.insert(pane, now);
        true
    }

    pub fn forget(&mut self, pane: PaneId) {
        self.sent.remove(&pane);
    }

    pub fn len(&self) -> usize {
        self.sent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
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
        .env_remove("TWITTY_SOCKET")
        .env_remove("TWITTY_PANE_ID")
        .env_remove("CMUX_SOCKET_PATH")
        .env_remove("CMUX_WORKSPACE_ID")
        .env_remove("CMUX_SURFACE_ID")
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    const MAX_INFLIGHT: usize = 8;
    static INFLIGHT: AtomicUsize = AtomicUsize::new(0);

    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            INFLIGHT.fetch_sub(1, Ordering::AcqRel);
        }
    }

    let mut cur = INFLIGHT.load(Ordering::Acquire);
    loop {
        if cur >= MAX_INFLIGHT {
            return;
        }
        match INFLIGHT.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
    let slot = Slot;
    if let Err(error) = std::thread::Builder::new()
        .name("twitty-notify".into())
        .spawn(move || {
            let _slot = slot;
            match command(&title, &body).spawn() {
                Ok(mut child) => {
                    const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);
                    let start = Instant::now();
                    loop {
                        match child.try_wait() {
                            Ok(Some(_)) => break,
                            Ok(None) if start.elapsed() >= NOTIFY_TIMEOUT => {
                                let _ = child.kill();
                                let _ = child.wait();
                                tracing::debug!("notify-send timed out and was stopped");
                                break;
                            }
                            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                            Err(error) => {
                                tracing::debug!(%error, "notify-send wait failed");
                                break;
                            }
                        }
                    }
                }
                Err(error) => tracing::debug!(%error, "desktop notification unavailable"),
            }
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
