# Product

## Register

product

## Users

Arabic-speaking developers and technical users who work in mixed Arabic/English shells on Windows and Linux. Their primary workflow is running normal commands, developer tools, and interactive terminal applications such as OpenCode without abandoning Arabic for prompts, filenames, logs, or conversation.

## Product Purpose

Twitty is a terminal-first desktop application that makes mixed-direction terminal text readable while preserving the behavior expected from a real PTY. Success means Arabic input and output remain correctly shaped and ordered, English commands and terminal control sequences remain stable, and users can trust cursor movement, selection, paste, resize, interrupts, and session recovery during daily work.

## Brand Personality

Quiet, trustworthy, and technically rigorous. Twitty should feel focused and native to experienced terminal users, with calm confidence rather than novelty or spectacle.

## Anti-references

- Flashy or gamified “AI terminal” interfaces with gradients, neon decoration, glass panels, and oversized chrome.
- Heavy IDE shells that bury the terminal beneath dashboards, sidebars, and decorative widgets.
- Full-page RTL treatments that reverse terminal geometry or make Latin commands unpredictable.
- Custom terminal behavior that looks correct in a demo but breaks standard shells, ANSI programs, cursor movement, or interactive TUIs.

## Design Principles

- The terminal is the task; surrounding UI exists only to clarify state or enable recovery.
- Arabic correctness is functional correctness, not decoration.
- Preserve earned terminal conventions before inventing new interaction patterns.
- Fail visibly and recover deliberately; never hide PTY or rendering failures behind a blank screen.
- Validate mixed-direction behavior in real workflows, not only isolated strings.

## Accessibility & Inclusion

Target WCAG 2.2 AA for application chrome, visible keyboard focus, screen-reader announcements for connection state, reduced-motion support, and status communication that does not rely on color alone. Arabic and Latin text must remain legible at common desktop scale factors, and all core actions must be keyboard accessible.
