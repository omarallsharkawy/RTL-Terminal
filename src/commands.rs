//! Every Twitty command by a stable name, for the command palette, the control socket
//! (`twitty ctl`) and anything else that triggers actions without a key press.
//!
//! The names are a public interface: scripts and agents call them, so a name, once shipped,
//! is never renamed or reused. Shortcuts live in `input::keys`; the code that runs a command
//! is `App::run_action`.

use crate::input::InputAction;
use crate::layout::FocusDir;

/// One command as the palette lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Stable name, e.g. `split.right`.
    pub name: &'static str,
    pub title_en: &'static str,
    pub title_ar: &'static str,
    /// Shortcut as shown to the user, if it has one.
    pub shortcut: Option<&'static str>,
}

macro_rules! commands {
    ($( $name:literal, $en:literal, $ar:literal, $keys:expr => $action:expr; )*) => {
        /// Every command, in palette order.
        pub fn all() -> Vec<Command> {
            vec![$(Command { name: $name, title_en: $en, title_ar: $ar, shortcut: $keys }),*]
        }

        /// The action a command name runs.
        pub fn action(name: &str) -> Option<InputAction> {
            match name {
                $($name => Some($action),)*
                _ => None,
            }
        }
    };
}

commands! {
    "palette.open", "Command palette", "لوحة الأوامر", Some("Ctrl+Shift+P") => InputAction::CommandPalette;
    "composer.open", "Compose command", "اكتب أمر", Some("Ctrl+Shift+Space") => InputAction::ToggleComposer;
    "settings.open", "Settings", "الإعدادات", Some("Ctrl+,") => InputAction::Settings;
    "memory.open", "Project memory", "ذاكرة المشروع", None => InputAction::ProjectMemory;
    "welcome.open", "Welcome", "الترحيب", None => InputAction::Welcome;
    "tab.new", "New tab", "تاب جديد", Some("Ctrl+Shift+T") => InputAction::NewTab;
    "tab.next", "Next tab", "التاب التالي", Some("Ctrl+Tab") => InputAction::NextTab;
    "tab.prev", "Previous tab", "التاب السابق", Some("Ctrl+Shift+Tab") => InputAction::PrevTab;
    "tab.move-left", "Move tab left", "حرّك التاب شمال", Some("Ctrl+Shift+PageUp") => InputAction::MoveTabLeft;
    "tab.move-right", "Move tab right", "حرّك التاب يمين", Some("Ctrl+Shift+PageDown") => InputAction::MoveTabRight;
    "tab.reopen", "Reopen closed tab", "رجّع آخر تاب اتقفل", Some("Ctrl+Shift+Alt+T") => InputAction::ReopenClosedTab;
    "tab.rename", "Rename tab…", "غيّر اسم التاب…", Some("Ctrl+Shift+N") => InputAction::RenameTab;
    "project.open", "Open project", "افتح مشروع", Some("Ctrl+Shift+L") => InputAction::OpenProject;
    "layout.save", "Save layout…", "احفظ التخطيط…", None => InputAction::SaveLayout;
    "layout.open", "Open layout…", "افتح تخطيط…", None => InputAction::OpenLayout;
    "layout.delete", "Delete layout…", "احذف تخطيط…", None => InputAction::DeleteLayout;
    "input.broadcast", "Type in all panes of this tab", "اكتب في كل اللوحات", None => InputAction::ToggleBroadcast;
    "pane.zoom", "Toggle pane zoom", "كبّر اللوحة أو رجّع حجمها", Some("Ctrl+Shift+Z") => InputAction::TogglePaneZoom;
    "pane.flash", "Flash focused pane", "ومّض إطار اللوحة النشطة", Some("Ctrl+Shift+H") => InputAction::FlashPane;
    "pane.close-others", "Close other panes", "اقفل باقي اللوحات", Some("Ctrl+Shift+K") => InputAction::CloseOtherPanes;
    "pane.close", "Close pane", "قفل اللوحة", Some("Ctrl+Shift+W") => InputAction::ClosePane;
    "split.right", "Split right", "قسّم يمين", Some("Ctrl+Shift+E") => InputAction::SplitRight;
    "split.down", "Split down", "قسّم تحت", Some("Ctrl+Shift+O") => InputAction::SplitDown;
    "focus.left", "Focus pane on the left", "روح للوحة الشمال", Some("Alt+Left") => InputAction::Focus(FocusDir::Left);
    "focus.right", "Focus pane on the right", "روح للوحة اليمين", Some("Alt+Right") => InputAction::Focus(FocusDir::Right);
    "focus.up", "Focus pane above", "روح للوحة اللي فوق", Some("Alt+Up") => InputAction::Focus(FocusDir::Up);
    "focus.down", "Focus pane below", "روح للوحة اللي تحت", Some("Alt+Down") => InputAction::Focus(FocusDir::Down);
    "sidebar.toggle", "Toggle sidebar", "إظهار/إخفاء الشريط الجانبي", Some("Ctrl+Shift+B") => InputAction::ToggleSidebar;
    "attention.next", "Go to next notification", "روح للإشعار اللي بعده", Some("Ctrl+Shift+U") => InputAction::NextAttention;
    "notifications.list", "Notifications", "الإشعارات", Some("Ctrl+Shift+I") => InputAction::NotificationList;
    "notifications.jump-unread", "Jump to newest unread notification", "روح لأحدث إشعار غير مقروء", Some("Ctrl+Shift+J") => InputAction::JumpUnread;
    "block.prev", "Previous command block", "البلوك اللي قبله", Some("Ctrl+Shift+Up") => InputAction::PrevBlock;
    "block.next", "Next command block", "البلوك اللي بعده", Some("Ctrl+Shift+Down") => InputAction::NextBlock;
    "block.copy-output", "Copy command output", "انسخ ناتج الأمر", Some("Ctrl+Shift+Y") => InputAction::CopyBlockOutput;
    "ai.explain-error", "Explain error", "اشرح الخطأ", None => InputAction::ExplainBlockError;
    "ai.suggest-fix", "Suggest a fix", "اقترح حل", None => InputAction::SuggestBlockFix;
    "agent.ask", "Ask an agent about this output", "اسأل وكيل عن الناتج ده", Some("Ctrl+Shift+A") => InputAction::AskAgent;
    "review.toggle", "Review changes", "راجع التغييرات", Some("Ctrl+Shift+R") => InputAction::ToggleReview;
    "edit.copy", "Copy", "نسخ", Some("Ctrl+Shift+C") => InputAction::Copy;
    "edit.paste", "Paste", "لصق", Some("Ctrl+Shift+V") => InputAction::Paste;
    "search.open", "Find in scrollback", "بحث", Some("Ctrl+Shift+F") => InputAction::Search;
    "search.all", "Search all terminals", "دوّر في كل الترمينالات", Some("Ctrl+Shift+G") => InputAction::SearchAll;
    "zoom.in", "Zoom in", "تكبير", Some("Ctrl+=") => InputAction::ZoomIn;
    "zoom.out", "Zoom out", "تصغير", Some("Ctrl+-") => InputAction::ZoomOut;
    "zoom.reset", "Reset zoom", "الحجم الأصلي", Some("Ctrl+0") => InputAction::ZoomReset;
}
