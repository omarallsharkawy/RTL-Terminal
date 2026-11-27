//! The windows that float over the panes, in one z-order.
//!
//! [`Layer::STACK`] is the only place that order is written. Keys, mouse presses and pointer
//! motion walk it top to bottom (`app::layers`), and the renderer draws it bottom to top, so
//! what is drawn on top is also what gets the input first. A new overlay is one variant here
//! plus one arm in each `match` that routes or draws it; the compiler lists every place that
//! still needs one.

/// One overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    /// Hover tooltip (display only).
    Tooltip,
    /// Short notices (display only).
    Toasts,
    /// Right-click menu.
    ContextMenu,
    /// One-time AI privacy consent (modal).
    AiConsent,
    /// First-run welcome (modal).
    Welcome,
    /// Command palette and its modes (modal).
    Palette,
    /// Project memory screen (modal).
    Memory,
    /// Settings (modal).
    Settings,
    /// Explanation or fix for a failed command, over its block.
    BlockAi,
    /// Command composer at the bottom of the focused pane.
    Composer,
    /// Docked agent side panel.
    AgentPanel,
    /// Docked code review panel.
    Review,
}

impl Layer {
    /// Top to bottom.
    pub const STACK: [Layer; 12] = [
        Layer::Tooltip,
        Layer::Toasts,
        Layer::ContextMenu,
        Layer::AiConsent,
        Layer::Welcome,
        Layer::Palette,
        Layer::Memory,
        Layer::Settings,
        Layer::BlockAi,
        Layer::Composer,
        Layer::AgentPanel,
        Layer::Review,
    ];

    /// Whether the layer covers the whole window and takes all input while open.
    pub fn is_modal(self) -> bool {
        matches!(
            self,
            Layer::ContextMenu
                | Layer::AiConsent
                | Layer::Welcome
                | Layer::Palette
                | Layer::Memory
                | Layer::Settings
        )
    }

    /// Bottom to top, the order to draw in.
    pub fn draw_order() -> impl Iterator<Item = Layer> {
        Self::STACK.into_iter().rev()
    }
}
