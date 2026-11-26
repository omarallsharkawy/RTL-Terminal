# Twitty Screenshots

This directory holds the release screenshots referenced by the root [README.md](../../README.md).

## Expected files

| File | Scene |
| --- | --- |
| `twitty-dark.png` | Twitty Dark workspace with Arabic/English output, command blocks, sidebar, and top bar |
| `twitty-light.png` | Twitty Light theme showing theme-derived chrome and clear Arabic letterforms |
| `arabic-rtl.png` | Arabic UI layout (1.5× HiDPI) with right-to-left mixed text, harakat, and box-drawn tables |
| `review-and-composer.png` | Side-by-side Git review panel and multi-line command composer with completions |

## Generating offscreen captures

You can render reference captures without a display server using the offscreen showcase test:

```bash
TWITTY_SHOWCASE_DIR=/tmp/twitty-showcase cargo test --test showcase_tests
```

Add or update the PNG files in this directory before tagging a public release.
