//! The overlay z-order is written once, holds every overlay exactly once, and is drawn in
//! reverse of the input order.
use std::collections::HashSet;
use twitty::ui::layers::Layer;

/// Every variant. The match fails to compile when a new overlay is added without being listed.
fn every_layer() -> Vec<Layer> {
    let all = [
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
    for layer in all {
        match layer {
            Layer::Tooltip
            | Layer::Toasts
            | Layer::ContextMenu
            | Layer::AiConsent
            | Layer::Welcome
            | Layer::Palette
            | Layer::Memory
            | Layer::Settings
            | Layer::BlockAi
            | Layer::Composer
            | Layer::AgentPanel
            | Layer::Review => {}
        }
    }
    all.to_vec()
}

#[test]
fn stack_lists_every_overlay_once() {
    let stack: HashSet<_> = Layer::STACK.into_iter().collect();
    assert_eq!(stack.len(), Layer::STACK.len());
    assert_eq!(stack, every_layer().into_iter().collect());
}

#[test]
fn drawing_goes_bottom_to_top() {
    let drawn: Vec<_> = Layer::draw_order().collect();
    let mut top_first = drawn.clone();
    top_first.reverse();
    assert_eq!(top_first, Layer::STACK);
    assert_eq!(drawn.last(), Some(&Layer::Tooltip));
}

#[test]
fn modal_overlays_sit_above_docked_ones() {
    let first_docked = Layer::STACK
        .iter()
        .position(|l| {
            matches!(
                l,
                Layer::BlockAi | Layer::Composer | Layer::AgentPanel | Layer::Review
            )
        })
        .unwrap();
    assert!(Layer::STACK[first_docked..].iter().all(|l| !l.is_modal()));
    // The right-click menu is above every other modal so it closes before them.
    let menu = Layer::STACK
        .iter()
        .position(|l| *l == Layer::ContextMenu)
        .unwrap();
    assert!(Layer::STACK[..menu].iter().all(|l| !l.is_modal()));
}
